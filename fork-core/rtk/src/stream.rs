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

    let mut child = ChildGuard(cmd.spawn().context("Failed to spawn process")?);
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
    let output = cmd.output().context("Failed to execute command")?;
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
