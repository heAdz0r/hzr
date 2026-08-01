use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::error::{MemoryError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitStatus {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitSnapshot {
    pub status: CircuitStatus,
    pub consecutive_failures: u32,
    pub retry_after: Option<Duration>,
}

#[derive(Debug)]
enum State {
    Closed { failures: u32, generation: u64 },
    Open { until: Instant, generation: u64 },
    HalfOpen { generation: u64 },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CircuitPermit {
    generation: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct CircuitBreaker {
    threshold: u32,
    reset_timeout: Duration,
    state: Arc<Mutex<State>>,
}

impl CircuitBreaker {
    pub(crate) fn new(threshold: u32, reset_timeout: Duration) -> Self {
        Self {
            threshold,
            reset_timeout,
            state: Arc::new(Mutex::new(State::Closed {
                failures: 0,
                generation: 0,
            })),
        }
    }

    pub(crate) async fn before_request(&self) -> Result<CircuitPermit> {
        let now = Instant::now();
        let mut state = self.state.lock().await;
        match &*state {
            State::Closed { generation, .. } => Ok(CircuitPermit {
                generation: *generation,
            }),
            State::Open { until, generation } if *until <= now => {
                let generation = *generation;
                *state = State::HalfOpen { generation };
                Ok(CircuitPermit { generation })
            }
            State::Open { until, .. } => Err(MemoryError::CircuitOpen {
                retry_after: until.saturating_duration_since(now),
            }),
            State::HalfOpen { .. } => Err(MemoryError::CircuitOpen {
                retry_after: self.reset_timeout,
            }),
        }
    }

    pub(crate) async fn record_success(&self, permit: CircuitPermit) {
        let mut state = self.state.lock().await;
        match &mut *state {
            State::Closed {
                failures,
                generation,
            } if *generation == permit.generation => *failures = 0,
            State::HalfOpen { generation } if *generation == permit.generation => {
                *state = State::Closed {
                    failures: 0,
                    generation: generation.saturating_add(1),
                };
            }
            State::Closed { .. } | State::Open { .. } | State::HalfOpen { .. } => {}
        }
    }

    pub(crate) async fn record_failure(&self, permit: CircuitPermit) {
        let mut state = self.state.lock().await;
        match &mut *state {
            State::Closed {
                failures,
                generation,
            } if *generation == permit.generation => {
                *failures = failures.saturating_add(1);
                if *failures >= self.threshold {
                    *state = State::Open {
                        until: Instant::now() + self.reset_timeout,
                        generation: generation.saturating_add(1),
                    };
                }
            }
            State::HalfOpen { generation } if *generation == permit.generation => {
                *state = State::Open {
                    until: Instant::now() + self.reset_timeout,
                    generation: generation.saturating_add(1),
                };
            }
            State::Closed { .. } | State::Open { .. } | State::HalfOpen { .. } => {}
        }
    }

    pub(crate) async fn snapshot(&self) -> CircuitSnapshot {
        let now = Instant::now();
        match &*self.state.lock().await {
            State::Closed { failures, .. } => CircuitSnapshot {
                status: CircuitStatus::Closed,
                consecutive_failures: *failures,
                retry_after: None,
            },
            State::Open { until, .. } => CircuitSnapshot {
                status: CircuitStatus::Open,
                consecutive_failures: self.threshold,
                retry_after: Some(until.saturating_duration_since(now)),
            },
            State::HalfOpen { .. } => CircuitSnapshot {
                status: CircuitStatus::HalfOpen,
                consecutive_failures: self.threshold,
                retry_after: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CircuitBreaker, CircuitStatus};

    #[tokio::test]
    async fn test_breaker_opens_at_threshold_and_recovers_half_open() {
        let breaker = CircuitBreaker::new(2, std::time::Duration::from_millis(1));
        let first = breaker.before_request().await.expect("first permit");
        breaker.record_failure(first).await;
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Closed);

        let second = breaker.before_request().await.expect("second permit");
        breaker.record_failure(second).await;
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Open);
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        let probe = breaker.before_request().await.expect("half-open probe");
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::HalfOpen);
        breaker.record_success(probe).await;
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Closed);
    }

    #[tokio::test]
    async fn test_breaker_allows_only_one_half_open_probe() {
        let breaker = CircuitBreaker::new(1, std::time::Duration::from_millis(1));
        let permit = breaker.before_request().await.expect("closed permit");
        breaker.record_failure(permit).await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        assert!(breaker.before_request().await.is_ok());
        assert!(breaker.before_request().await.is_err());
    }

    #[tokio::test]
    async fn test_stale_success_cannot_close_newer_open_generation() {
        let breaker = CircuitBreaker::new(1, std::time::Duration::from_secs(30));
        let stale_success = breaker.before_request().await.expect("stale permit");
        let failure = breaker.before_request().await.expect("failure permit");

        breaker.record_failure(failure).await;
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Open);
        breaker.record_success(stale_success).await;

        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Open);
    }
}
