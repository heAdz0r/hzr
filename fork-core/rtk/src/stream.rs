use anyhow::{Context, Result};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// Read `reader` line by line, decoding lossily instead of erroring.
///
/// `BufRead::lines()` yields `Err` for a line that is not valid UTF-8, and the
/// idiomatic `.map_while(Result::ok)` chain stops the iterator at that first
/// `Err` — so a single stray byte (a latin-1 filename, a binary blob in a test
/// log, OEM bytes from a non-English-locale tool) silently discarded *every
/// remaining line of that stream*, and the loss was then recorded as a saving.
/// Splitting on the raw byte and decoding each line with `from_utf8_lossy`
/// keeps the garbled line visible and, more importantly, keeps everything
/// after it.
fn read_lines_lossy(reader: impl Read) -> impl Iterator<Item = String> {
    BufReader::new(reader).split(b'\n').filter_map(|res| {
        let mut buf = match res {
            Ok(buf) => buf,
            Err(e) => {
                eprintln!("[rtk] warning: stream read error: {}", e);
                return None;
            }
        };
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    })
}

pub trait StreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String>;
    fn flush(&mut self) -> String;
    fn on_exit(&mut self, _exit_code: i32, _raw: &str) -> Option<String> {
        None
    }
}

pub enum FilterMode<'a> {
    Streaming(Box<dyn StreamFilter + 'a>),
    Passthrough,
    /// Capture both streams under `RAW_CAP` without filtering or echoing.
    ///
    /// The alternative — `Command::output()` — buffers the child's entire stdout
    /// and then copies it again into a `String`. A search over a large tree has
    /// emitted gigabytes that way and OOM-killed the calling agent, and a
    /// downstream `| head -N` is no escape because nothing is written until the
    /// child exits.
    CaptureOnly,
}

pub enum StdinMode {
    Inherit,
}

pub struct StreamResult {
    pub exit_code: i32,
    pub raw_stdout: String,
    pub raw_stderr: String,
    pub filtered: String,
}

// 0.11.0 (US-007, upstream 9900d70/5b1d523/9418aa8): while rtk holds a child's
// captured output, SIGINT/SIGTERM is relayed to the child instead of killing rtk
// on the spot. The child ends, rtk prints what it captured, then dies by the same
// signal so the caller still sees a signal death. A child that ignores the relay
// is SIGKILLed after a grace period; an inherited SIG_IGN is left in place.
#[cfg(unix)]
mod signal_relay {
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
    use std::thread;
    use std::time::Duration;

    const POLL: Duration = Duration::from_millis(25);
    const KILL_GRACE: Duration = Duration::from_millis(750);
    const EXIT_GRACE: Duration = Duration::from_millis(750);

    static CHILD_PID: AtomicU32 = AtomicU32::new(0);
    static RELAYED: AtomicI32 = AtomicI32::new(0);
    static FINISHED: AtomicBool = AtomicBool::new(false);
    /// The child leads its own process group: signal the whole group, so a
    /// grandchild holding the output pipe (`sh -c script` → `sleep`) ends too.
    static GROUP: AtomicBool = AtomicBool::new(false);

    fn target(pid: u32) -> libc::pid_t {
        if GROUP.load(Ordering::SeqCst) {
            -(pid as libc::pid_t)
        } else {
            pid as libc::pid_t
        }
    }

    /// Signal handler: only atomics, `kill`, `signal` and `raise`, all
    /// async-signal-safe.
    extern "C" fn relay(sig: libc::c_int) {
        let pid = CHILD_PID.load(Ordering::SeqCst);
        if pid == 0 || RELAYED.swap(sig, Ordering::SeqCst) != 0 {
            // No child, or a second signal: default action, now.
            // SAFETY: async-signal-safe libc calls inside a signal handler.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
                libc::raise(sig);
            }
            return;
        }
        // SAFETY: async-signal-safe; the pid is our own live child (or its group).
        unsafe {
            libc::kill(target(pid), sig);
        }
    }

    fn escalate(pid: u32) {
        thread::sleep(KILL_GRACE);
        if FINISHED.load(Ordering::SeqCst) {
            return;
        }
        // SAFETY: plain libc call on our own child (or its group).
        unsafe {
            libc::kill(target(pid), libc::SIGKILL);
        }
        thread::sleep(EXIT_GRACE);
        if FINISHED.load(Ordering::SeqCst) {
            return;
        }
        let sig = RELAYED.load(Ordering::SeqCst);
        // SAFETY: restoring the default action and re-raising ends the process.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    pub fn relayed() -> Option<libc::c_int> {
        match RELAYED.load(Ordering::SeqCst) {
            0 => None,
            sig => Some(sig),
        }
    }

    pub struct Relay;

    impl Relay {
        /// `group`: the child was spawned as the leader of its own process group.
        pub fn install(pid: u32, group: bool) -> Self {
            GROUP.store(group, Ordering::SeqCst);
            CHILD_PID.store(pid, Ordering::SeqCst);
            FINISHED.store(false, Ordering::SeqCst);
            // SAFETY: installing a handler that only makes async-signal-safe calls.
            unsafe {
                for sig in [libc::SIGINT, libc::SIGTERM] {
                    let handler = relay as extern "C" fn(libc::c_int) as libc::sighandler_t;
                    let previous = libc::signal(sig, handler);
                    if previous == libc::SIG_IGN {
                        // `nohup`/background jobs ignore SIGINT on purpose.
                        libc::signal(sig, libc::SIG_IGN);
                    }
                }
            }
            thread::spawn(move || {
                while !FINISHED.load(Ordering::SeqCst) {
                    if RELAYED.load(Ordering::SeqCst) != 0 {
                        escalate(pid);
                        return;
                    }
                    thread::sleep(POLL);
                }
            });
            Relay
        }
    }

    impl Drop for Relay {
        fn drop(&mut self) {
            FINISHED.store(true, Ordering::SeqCst);
            CHILD_PID.store(0, Ordering::SeqCst);
            // SAFETY: restoring default dispositions, preserving an inherited SIG_IGN.
            unsafe {
                for sig in [libc::SIGINT, libc::SIGTERM] {
                    if libc::signal(sig, libc::SIG_DFL) == libc::SIG_IGN {
                        libc::signal(sig, libc::SIG_IGN);
                    }
                }
            }
        }
    }
}

#[cfg(not(unix))]
mod signal_relay {
    pub struct Relay;

    impl Relay {
        pub fn install(_pid: u32, _group: bool) -> Self {
            Relay
        }
    }
}

/// After the captured output is printed, die by the signal that was relayed, if
/// any, so the caller sees `128 + signal` from a real signal death. (0.11.0, US-007)
#[cfg(unix)]
pub fn die_by_relayed_signal() {
    let Some(sig) = signal_relay::relayed() else {
        return;
    };
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    // SAFETY: restoring the default action and re-raising ends the process.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

#[cfg(not(unix))]
pub fn die_by_relayed_signal() {}

/// Spawn the child as leader of its own process group so a relayed signal reaches
/// its whole tree (`sh -c script` → a grandchild holding the output pipe). Only
/// when none of rtk's own stdio is a terminal — the agent case: at an interactive
/// terminal a background group that prompts on the TTY (`prisma migrate dev`)
/// would be stopped, so there the relay targets the child alone, as upstream
/// does. Returns whether the group was requested. (0.11.0, US-007)
fn own_process_group(cmd: &mut Command) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: isatty only inspects file descriptors.
        let interactive = (0..3).any(|fd| unsafe { libc::isatty(fd) } == 1);
        if !interactive {
            cmd.process_group(0);
        }
        !interactive
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
        false
    }
}

/// `Command::output()` with the signal relay installed for the child's lifetime.
/// Same stdio as `output()`: stdin null, stdout and stderr captured.
/// (0.11.0, US-007)
pub trait RelayedOutput {
    fn output_relayed(&mut self) -> io::Result<std::process::Output>;
}

impl RelayedOutput for Command {
    fn output_relayed(&mut self) -> io::Result<std::process::Output> {
        let group = own_process_group(self);
        let child = self
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let _signal_relay = signal_relay::Relay::install(child.id(), group);
        child.wait_with_output()
    }
}

pub fn status_to_exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

pub const RAW_CAP: usize = 10_485_760;

pub fn run_streaming(
    cmd: &mut Command,
    stdin_mode: StdinMode,
    stdout_mode: FilterMode<'_>,
) -> Result<StreamResult> {
    if matches!(stdout_mode, FilterMode::Passthrough) {
        match stdin_mode {
            StdinMode::Inherit => cmd.stdin(Stdio::inherit()),
        };
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
        let status = cmd.status().context("Failed to spawn process")?;
        return Ok(StreamResult {
            exit_code: status_to_exit_code(status),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            filtered: String::new(),
        });
    }

    match stdin_mode {
        StdinMode::Inherit => cmd.stdin(Stdio::inherit()),
    };
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.0.wait().ok();
        }
    }

    let group = own_process_group(cmd); // 0.11.0 (US-007)
    let mut child = ChildGuard(cmd.spawn().context("Failed to spawn process")?);
    let _signal_relay = signal_relay::Relay::install(child.0.id(), group); // 0.11.0 (US-007)
    let stdout = child.0.stdout.take().context("No child stdout handle")?;
    let stderr = child.0.stderr.take().context("No child stderr handle")?;

    enum StreamLine {
        Stdout(String),
        Stderr(String),
    }

    let (tx, rx) = mpsc::channel();
    let tx_out = tx.clone();
    let stdout_thread = std::thread::spawn(move || {
        for line in read_lines_lossy(stdout) {
            if tx_out.send(StreamLine::Stdout(line)).is_err() {
                break;
            }
        }
    });
    let stderr_thread = std::thread::spawn(move || {
        for line in read_lines_lossy(stderr) {
            if tx.send(StreamLine::Stderr(line)).is_err() {
                break;
            }
        }
    });

    let mut raw_stdout = String::new();
    let mut raw_stderr = String::new();
    let mut filtered = String::new();
    let mut capped_out = false;
    let mut capped_err = false;
    let mut filter_fd_is_stderr = false;
    let mut saved_filter: Option<Box<dyn StreamFilter + '_>> = None;

    if let FilterMode::Streaming(mut filter) = stdout_mode {
        let stdout_handle = io::stdout();
        let mut out = stdout_handle.lock();
        let stderr_handle = io::stderr();
        let mut err_out = stderr_handle.lock();

        for msg in rx {
            let (line, is_stderr) = match msg {
                StreamLine::Stderr(line) => (line, true),
                StreamLine::Stdout(line) => (line, false),
            };
            if is_stderr {
                if !capped_err {
                    if raw_stderr.len() + line.len() < RAW_CAP {
                        raw_stderr.push_str(&line);
                        raw_stderr.push('\n');
                    } else {
                        capped_err = true;
                        eprintln!("[rtk] warning: stderr exceeds 10 MiB - capture truncated");
                    }
                }
            } else if !capped_out {
                if raw_stdout.len() + line.len() < RAW_CAP {
                    raw_stdout.push_str(&line);
                    raw_stdout.push('\n');
                } else {
                    capped_out = true;
                    eprintln!("[rtk] warning: stdout exceeds 10 MiB - filter input truncated");
                }
            }

            filter_fd_is_stderr = is_stderr;
            if let Some(output) = filter.feed_line(&line) {
                filtered.push_str(&output);
                let dest: &mut dyn Write = if is_stderr { &mut err_out } else { &mut out };
                match write!(dest, "{}", output) {
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
                    Err(error) => return Err(error.into()),
                    Ok(_) => {}
                }
            }
        }

        let tail = filter.flush();
        filtered.push_str(&tail);
        let dest: &mut dyn Write = if filter_fd_is_stderr {
            &mut err_out
        } else {
            &mut out
        };
        match write!(dest, "{}", tail) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        saved_filter = Some(filter);
    } else if matches!(stdout_mode, FilterMode::CaptureOnly) {
        // Drain under the same cap, without filtering or echoing. Not draining
        // at all would block the reader threads on a full channel.
        for msg in rx {
            let (line, is_stderr) = match msg {
                StreamLine::Stderr(line) => (line, true),
                StreamLine::Stdout(line) => (line, false),
            };
            if is_stderr {
                if !capped_err {
                    if raw_stderr.len() + line.len() < RAW_CAP {
                        raw_stderr.push_str(&line);
                        raw_stderr.push('\n');
                    } else {
                        capped_err = true;
                        eprintln!("[rtk] warning: stderr exceeds 10 MiB - capture truncated");
                    }
                }
            } else if !capped_out {
                if raw_stdout.len() + line.len() < RAW_CAP {
                    raw_stdout.push_str(&line);
                    raw_stdout.push('\n');
                } else {
                    capped_out = true;
                    eprintln!("[rtk] warning: stdout exceeds 10 MiB - capture truncated");
                }
            }
        }
    }

    stdout_thread.join().ok();
    stderr_thread.join().ok();
    let status = child.0.wait().context("Failed to wait for child")?;
    let exit_code = status_to_exit_code(status);
    let raw = format!("{}{}", raw_stdout, raw_stderr);

    if let Some(mut filter) = saved_filter {
        if let Some(post) = filter.on_exit(exit_code, &raw) {
            filtered.push_str(&post);
            let mut dest: Box<dyn Write> = if filter_fd_is_stderr {
                Box::new(io::stderr().lock())
            } else {
                Box::new(io::stdout().lock())
            };
            match write!(dest, "{}", post) {
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
        }
    }

    Ok(StreamResult {
        exit_code,
        raw_stdout,
        raw_stderr,
        filtered,
    })
}

pub struct CaptureResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub fn exec_capture(cmd: &mut Command) -> Result<CaptureResult> {
    cmd.stdin(Stdio::null());
    capture(cmd)
}

pub fn exec_capture_stdin(cmd: &mut Command) -> Result<CaptureResult> {
    cmd.stdin(Stdio::inherit());
    capture(cmd)
}

/// Shared body of both capture helpers. Routing them through
/// `exit_code_from_output` rather than `status_to_exit_code` keeps the stderr
/// diagnostic that explains a signal death; both return the same `128 + signal`
/// code, but only one of them says *why*. The program name is the label, so no
/// call site has to supply one.
fn capture(cmd: &mut Command) -> Result<CaptureResult> {
    let label = cmd.get_program().to_string_lossy().into_owned();
    // 0.11.0 (US-007): spawn + wait so a signal to rtk is relayed while capturing.
    let group = own_process_group(cmd);
    let child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to execute command")?;
    let _signal_relay = signal_relay::Relay::install(child.id(), group);
    let output = child
        .wait_with_output()
        .context("Failed to execute command")?;
    Ok(CaptureResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: crate::utils::exit_code_from_output(&output, &label),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_zero() {
        let status = Command::new("true").status().unwrap();
        assert_eq!(status_to_exit_code(status), 0);
    }

    #[test]
    fn exit_code_nonzero() {
        let status = Command::new("false").status().unwrap();
        assert_eq!(status_to_exit_code(status), 1);
    }

    #[test]
    fn lossy_read_keeps_lines_after_invalid_utf8() {
        let input: &[u8] = b"first\n\xff\xfe bad\nafter\n";
        let lines: Vec<String> = read_lines_lossy(input).collect();
        assert_eq!(lines.len(), 3, "no line may be dropped: {:?}", lines);
        assert_eq!(lines[0], "first");
        assert!(lines[1].contains('\u{fffd}'), "bad line decodes lossily");
        assert_eq!(
            lines[2], "after",
            "everything after an invalid line must survive"
        );
    }

    #[test]
    fn lossy_read_strips_crlf_and_tolerates_missing_final_newline() {
        let input: &[u8] = b"a\r\nb";
        let lines: Vec<String> = read_lines_lossy(input).collect();
        assert_eq!(lines, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn lossy_read_empty_input_yields_nothing() {
        let input: &[u8] = b"";
        assert_eq!(read_lines_lossy(input).count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn streaming_keeps_output_after_invalid_utf8() {
        struct KeepAll;
        impl StreamFilter for KeepAll {
            fn feed_line(&mut self, line: &str) -> Option<String> {
                Some(format!("{}\n", line))
            }
            fn flush(&mut self) -> String {
                String::new()
            }
        }

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'ok\\n\\377\\376\\nafter\\n'; exit 3");
        let result = run_streaming(
            &mut cmd,
            StdinMode::Inherit,
            FilterMode::Streaming(Box::new(KeepAll)),
        )
        .expect("stream");

        assert_eq!(result.exit_code, 3);
        assert!(
            result.raw_stdout.contains("after"),
            "raw capture truncated at the invalid byte: {:?}",
            result.raw_stdout
        );
        assert!(
            result.filtered.contains("after"),
            "filtered output truncated at the invalid byte: {:?}",
            result.filtered
        );
    }

    #[cfg(unix)]
    #[test]
    fn exit_code_signal_kill() {
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status_to_exit_code(status), 137);
    }
}
