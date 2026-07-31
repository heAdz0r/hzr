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
    Closed { failures: u32 },
    Open { until: Instant },
    HalfOpen,
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
            state: Arc::new(Mutex::new(State::Closed { failures: 0 })),
        }
    }

    pub(crate) async fn before_request(&self) -> Result<()> {
        let now = Instant::now();
        let mut state = self.state.lock().await;
        match &*state {
            State::Closed { .. } => Ok(()),
            State::Open { until } if *until <= now => {
                *state = State::HalfOpen;
                Ok(())
            }
            State::Open { until } => Err(MemoryError::CircuitOpen {
                retry_after: until.saturating_duration_since(now),
            }),
            State::HalfOpen => Err(MemoryError::CircuitOpen {
                retry_after: self.reset_timeout,
            }),
        }
    }

    pub(crate) async fn record_success(&self) {
        *self.state.lock().await = State::Closed { failures: 0 };
    }

    pub(crate) async fn record_failure(&self) {
        let mut state = self.state.lock().await;
        match &mut *state {
            State::Closed { failures } => {
                *failures += 1;
                if *failures >= self.threshold {
                    *state = State::Open {
                        until: Instant::now() + self.reset_timeout,
                    };
                }
            }
            State::Open { until } => *until = Instant::now() + self.reset_timeout,
            State::HalfOpen => {
                *state = State::Open {
                    until: Instant::now() + self.reset_timeout,
                };
            }
        }
    }

    pub(crate) async fn snapshot(&self) -> CircuitSnapshot {
        let now = Instant::now();
        match &*self.state.lock().await {
            State::Closed { failures } => CircuitSnapshot {
                status: CircuitStatus::Closed,
                consecutive_failures: *failures,
                retry_after: None,
            },
            State::Open { until } => CircuitSnapshot {
                status: CircuitStatus::Open,
                consecutive_failures: self.threshold,
                retry_after: Some(until.saturating_duration_since(now)),
            },
            State::HalfOpen => CircuitSnapshot {
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
        breaker.record_failure().await;
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Closed);

        breaker.record_failure().await;
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Open);
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        assert!(breaker.before_request().await.is_ok());
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::HalfOpen);
        breaker.record_success().await;
        assert_eq!(breaker.snapshot().await.status, CircuitStatus::Closed);
    }

    #[tokio::test]
    async fn test_breaker_allows_only_one_half_open_probe() {
        let breaker = CircuitBreaker::new(1, std::time::Duration::from_millis(1));
        breaker.record_failure().await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        assert!(breaker.before_request().await.is_ok());
        assert!(breaker.before_request().await.is_err());
    }
}
