use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

enum Event {
    Step(String),
    Bytes(u64, Option<u64>),
}

/// Owns one stderr line; dropping it also clears the line on errors.
pub(super) struct Progress {
    sender: Option<SyncSender<Event>>,
    worker: Option<JoinHandle<()>>,
    enabled: bool,
}

impl Progress {
    pub(super) fn new(enabled: bool) -> Self {
        let animated = enabled
            && io::stderr().is_terminal()
            && std::env::var("TERM").is_ok_and(|term| term != "dumb");
        let (sender, worker) = if animated {
            let (sender, receiver) = mpsc::sync_channel(8);
            let worker = thread::spawn(move || {
                let started = Instant::now();
                let mut label = String::new();
                let mut bytes = None;
                let mut tick = 0;
                let mut last_draw = Instant::now();
                loop {
                    match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(Event::Step(next)) => {
                            if !label.is_empty() {
                                eprintln!("\r\x1b[2K  • {label}");
                            }
                            label = next;
                            bytes = None;
                        }
                        Ok(Event::Bytes(done, total)) => bytes = Some((done, total)),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    let complete = bytes.is_some_and(|(done, total)| total == Some(done));
                    if !complete && last_draw.elapsed() < Duration::from_millis(100) {
                        continue;
                    }
                    last_draw = Instant::now();
                    let frame = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][tick % 10];
                    tick += 1;
                    let detail = bytes.map_or_else(String::new, |(done, total)| {
                        download_line(done, total, started.elapsed())
                    });
                    eprint!("\r\x1b[2K  {frame} {label}{detail}");
                    let _ = io::stderr().flush();
                }
                if !label.is_empty() {
                    eprintln!("\r\x1b[2K  • {label}");
                }
                let _ = io::stderr().flush();
            });
            (Some(sender), Some(worker))
        } else {
            (None, None)
        };
        Self {
            sender,
            worker,
            enabled,
        }
    }

    pub(super) fn step(&self, label: impl Into<String>) {
        let label = label.into();
        if let Some(sender) = &self.sender {
            let _ = sender.send(Event::Step(label));
        } else if self.enabled {
            eprintln!("  • {label}");
        }
    }

    pub(super) fn bytes(&self, downloaded: u64, total: Option<u64>) {
        if let Some(sender) = &self.sender {
            if total == Some(downloaded) {
                let _ = sender.send(Event::Bytes(downloaded, total));
            } else {
                let _ = sender.try_send(Event::Bytes(downloaded, total));
            }
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn download_line(done: u64, total: Option<u64>, elapsed: Duration) -> String {
    let mib = done as f64 / 1_048_576.0;
    let speed = done as f64 / elapsed.as_secs_f64().max(0.001);
    match total.filter(|total| *total > 0) {
        Some(total) => {
            let ratio = (done as f64 / total as f64).min(1.0);
            let filled = (ratio * 16.0) as usize;
            let eta = (total.saturating_sub(done) as f64 / speed.max(1.0)) as u64;
            format!(
                " [{}{}] {:3.0}%  {mib:.1}/{:.1} MiB  {:.1} MiB/s  ~{}:{:02}",
                "━".repeat(filled),
                "─".repeat(16 - filled),
                ratio * 100.0,
                total as f64 / 1_048_576.0,
                speed / 1_048_576.0,
                eta / 60,
                eta % 60
            )
        }
        None => format!("  {mib:.1} MiB  {:.1} MiB/s", speed / 1_048_576.0),
    }
}
