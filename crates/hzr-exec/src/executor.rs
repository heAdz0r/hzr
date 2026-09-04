use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::capture::CaptureWriter;
use crate::{
    CanonicalCommand, CapturedStream, ExecError, ExecutionEnvelope, ExecutionEvent,
    ExecutionOutcome, ExecutionResult, ExecutionStream, NotStarted, RewriteDecision, StdinSpec,
    Termination, TerminationCause,
};

#[derive(Clone, Debug, Default)]
pub struct ExecutionPipeline;

impl ExecutionPipeline {
    pub fn start(&self, envelope: ExecutionEnvelope) -> Result<ExecutionHandle, ExecError> {
        envelope.capture.validate()?;
        match &envelope.decision {
            RewriteDecision::Ask { proposed, reason } => Ok(ExecutionHandle::immediate(
                ExecutionOutcome::NotStarted {
                    disposition: NotStarted::ApprovalRequired {
                        decision_id: None,
                        requested: envelope.command.clone(),
                        proposed: proposed.clone(),
                        reason: reason.clone(),
                    },
                },
                envelope.capture.event_buffer,
            )),
            RewriteDecision::Deny { reason } => Ok(ExecutionHandle::immediate(
                ExecutionOutcome::NotStarted {
                    disposition: NotStarted::Denied {
                        requested: envelope.command.clone(),
                        reason: reason.clone(),
                    },
                },
                envelope.capture.event_buffer,
            )),
            RewriteDecision::AllowRaw { .. } | RewriteDecision::AllowRewrite { .. } => {
                self.start_process(envelope)
            }
        }
    }

    pub async fn execute(
        &self,
        envelope: ExecutionEnvelope,
    ) -> Result<ExecutionOutcome, ExecError> {
        self.start(envelope)?.wait().await
    }

    fn start_process(&self, envelope: ExecutionEnvelope) -> Result<ExecutionHandle, ExecError> {
        let requested = envelope.command.clone();
        let preferred = match &envelope.decision {
            RewriteDecision::AllowRewrite { command, .. } => command.clone(),
            RewriteDecision::AllowRaw { .. } => requested.clone(),
            RewriteDecision::Ask { .. } | RewriteDecision::Deny { .. } => requested.clone(),
        };
        let raw_fallback_allowed = matches!(
            &envelope.decision,
            RewriteDecision::AllowRewrite {
                source: crate::RewriteSource::HzrPolicy,
                ..
            }
        );

        let (child, executed, raw_fallback) = match spawn_command(&preferred, &envelope) {
            Ok(child) => (child, preferred, false),
            Err(_) if raw_fallback_allowed => {
                let child = spawn_command(&requested, &envelope)?;
                (child, requested.clone(), true)
            }
            Err(error) => return Err(error),
        };

        let (event_tx, event_rx) = mpsc::channel(envelope.capture.event_buffer);
        let (completion_tx, completion_rx) = oneshot::channel();
        let cancellation = Cancellation::default();
        let completion_cancellation = cancellation.clone();
        let completion = tokio::spawn(async move {
            let result = run_process(
                child,
                envelope,
                requested,
                executed,
                raw_fallback,
                completion_cancellation,
                event_tx,
            )
            .await;
            let _ = completion_tx.send(result);
        });

        Ok(ExecutionHandle {
            events: event_rx,
            completion: Some(completion_rx),
            task: Some(completion),
            cancellation,
        })
    }
}

pub struct ExecutionHandle {
    events: mpsc::Receiver<ExecutionEvent>,
    completion: Option<oneshot::Receiver<Result<ExecutionOutcome, ExecError>>>,
    task: Option<JoinHandle<()>>,
    cancellation: Cancellation,
}

impl ExecutionHandle {
    fn immediate(outcome: ExecutionOutcome, event_buffer: usize) -> Self {
        let (_event_tx, event_rx) = mpsc::channel(event_buffer);
        let (completion_tx, completion_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = completion_tx.send(Ok(outcome));
        });
        Self {
            events: event_rx,
            completion: Some(completion_rx),
            task: Some(task),
            cancellation: Cancellation::default(),
        }
    }

    pub async fn next_event(&mut self) -> Option<ExecutionEvent> {
        self.events.recv().await
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn cancellation(&self) -> ExecutionCancellation {
        ExecutionCancellation(self.cancellation.clone())
    }

    pub async fn wait(mut self) -> Result<ExecutionOutcome, ExecError> {
        let completion = self.completion.take().ok_or(ExecError::CompletionClosed)?;
        let task = self.task.take().ok_or(ExecError::CompletionClosed)?;
        let outcome = completion.await.map_err(|_| ExecError::CompletionClosed)?;
        task.await.map_err(|source| ExecError::Join {
            task: "execution",
            source,
        })?;
        outcome
    }
}

impl Drop for ExecutionHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionCancellation(Cancellation);

impl ExecutionCancellation {
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

#[derive(Clone, Debug, Default)]
struct Cancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Cancellation {
    fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

fn spawn_command(
    command: &CanonicalCommand,
    envelope: &ExecutionEnvelope,
) -> Result<Child, ExecError> {
    let mut process = match command {
        CanonicalCommand::Argv { program, args } => {
            if program.is_empty() {
                return Err(ExecError::EmptyProgram);
            }
            let mut process = Command::new(program);
            process.args(args);
            process
        }
        CanonicalCommand::Shell { shell, command } => {
            if shell.is_empty() {
                return Err(ExecError::EmptyShell);
            }
            let mut process = Command::new(shell);
            #[cfg(unix)]
            process.arg("-c").arg(command);
            #[cfg(windows)]
            process.arg("/C").arg(command);
            process
        }
    };
    process
        .stdin(match &envelope.stdin {
            StdinSpec::Null => Stdio::null(),
            StdinSpec::Bytes { .. } => Stdio::piped(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &envelope.cwd {
        process.current_dir(cwd);
    }
    if !envelope.environment.inherit {
        process.env_clear();
    }
    process.envs(&envelope.environment.set);
    for key in &envelope.environment.remove {
        process.env_remove(key);
    }
    configure_process_group(&mut process);
    process.spawn().map_err(|source| ExecError::Spawn {
        program: command.program().to_owned(),
        source,
    })
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

async fn run_process(
    mut child: Child,
    envelope: ExecutionEnvelope,
    requested: CanonicalCommand,
    executed: CanonicalCommand,
    raw_fallback: bool,
    cancellation: Cancellation,
    event_tx: mpsc::Sender<ExecutionEvent>,
) -> Result<ExecutionOutcome, ExecError> {
    let started = Instant::now();
    let pid = child.id();
    let mut process_group = ProcessGroupGuard::new(pid);
    let _ = event_tx.try_send(ExecutionEvent::Started {
        pid,
        command: executed.clone(),
    });

    let stdout = child.stdout.take().ok_or_else(|| ExecError::MissingPipe {
        program: executed.program().to_owned(),
        stream: "stdout",
    })?;
    let stderr = child.stderr.take().ok_or_else(|| ExecError::MissingPipe {
        program: executed.program().to_owned(),
        stream: "stderr",
    })?;
    let stdout_task = tokio::spawn(capture_stream(
        stdout,
        envelope.capture.clone(),
        ExecutionStream::Stdout,
        event_tx.clone(),
    ));
    let stderr_task = tokio::spawn(capture_stream(
        stderr,
        envelope.capture.clone(),
        ExecutionStream::Stderr,
        event_tx.clone(),
    ));
    let stdin_task = start_stdin_writer(&mut child, &envelope.stdin, executed.program())?;

    let (cause, status) = wait_for_termination(
        &mut child,
        envelope.timeout(),
        envelope.termination_grace(),
        &cancellation,
        executed.program(),
    )
    .await?;
    process_group.disarm();
    if let Some(stdin_task) = stdin_task {
        stdin_task.await.map_err(|source| ExecError::Join {
            task: "stdin",
            source,
        })??;
    }
    let stdout = join_capture(stdout_task, "stdout").await?;
    let stderr = join_capture(stderr_task, "stderr").await?;
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let termination = termination(cause, status);
    let _ = event_tx.try_send(ExecutionEvent::Finished {
        termination: termination.clone(),
        duration_ms,
    });

    Ok(ExecutionOutcome::Completed {
        result: Box::new(ExecutionResult {
            requested,
            executed,
            decision: envelope.decision,
            raw_fallback,
            stdout,
            stderr,
            termination,
            duration_ms,
        }),
    })
}

#[cfg(unix)]
struct ProcessGroupGuard {
    pid: Option<i32>,
}

#[cfg(unix)]
impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self {
            pid: pid.and_then(|pid| i32::try_from(pid).ok()),
        }
    }

    fn disarm(&mut self) {
        self.pid = None;
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;

        if let Some(pid) = self.pid {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
struct ProcessGroupGuard;

#[cfg(not(unix))]
impl ProcessGroupGuard {
    fn new(_pid: Option<u32>) -> Self {
        Self
    }

    fn disarm(&mut self) {}
}

fn start_stdin_writer(
    child: &mut Child,
    stdin: &StdinSpec,
    program: &str,
) -> Result<Option<JoinHandle<Result<(), ExecError>>>, ExecError> {
    let StdinSpec::Bytes { data } = stdin else {
        return Ok(None);
    };
    let mut child_stdin = child.stdin.take().ok_or_else(|| ExecError::MissingPipe {
        program: program.to_owned(),
        stream: "stdin",
    })?;
    let data = data.clone();
    let program = program.to_owned();
    Ok(Some(tokio::spawn(async move {
        match child_stdin.write_all(&data).await {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(source) => Err(ExecError::WriteStdin { program, source }),
        }
    })))
}

async fn capture_stream<R>(
    mut reader: R,
    config: crate::CaptureConfig,
    stream: ExecutionStream,
    event_tx: mpsc::Sender<ExecutionEvent>,
) -> Result<CapturedStream, ExecError>
where
    R: AsyncRead + Unpin,
{
    let mut capture = CaptureWriter::new(config, stream);
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|source| ExecError::Wait {
                program: format!("{stream:?} capture"),
                source,
            })?;
        if read == 0 {
            break;
        }
        let bytes = &buffer[..read];
        if let Err(error) = event_tx.try_send(ExecutionEvent::Output {
            stream,
            bytes: bytes.to_vec(),
        }) {
            if matches!(error, mpsc::error::TrySendError::Full(_)) {
                capture.record_dropped_event(read);
            }
        }
        capture.push(bytes).await?;
    }
    capture.finish().await
}

async fn join_capture(
    task: JoinHandle<Result<CapturedStream, ExecError>>,
    name: &'static str,
) -> Result<CapturedStream, ExecError> {
    task.await
        .map_err(|source| ExecError::Join { task: name, source })?
}

async fn wait_for_termination(
    child: &mut Child,
    timeout: Option<Duration>,
    grace: Duration,
    cancellation: &Cancellation,
    program: &str,
) -> Result<(TerminationCause, ExitStatus), ExecError> {
    if let Some(timeout) = timeout {
        tokio::select! {
            status = child.wait() => status
                .map(|status| (natural_cause(&status), status))
                .map_err(|source| ExecError::Wait { program: program.to_owned(), source }),
            () = cancellation.cancelled() => {
                terminate(child, grace, program).await.map(|status| (TerminationCause::Cancelled, status))
            }
            () = tokio::time::sleep(timeout) => {
                terminate(child, grace, program).await.map(|status| (TerminationCause::TimedOut, status))
            }
        }
    } else {
        tokio::select! {
            status = child.wait() => status
                .map(|status| (natural_cause(&status), status))
                .map_err(|source| ExecError::Wait { program: program.to_owned(), source }),
            () = cancellation.cancelled() => {
                terminate(child, grace, program).await.map(|status| (TerminationCause::Cancelled, status))
            }
        }
    }
}

async fn terminate(
    child: &mut Child,
    grace: Duration,
    program: &str,
) -> Result<ExitStatus, ExecError> {
    send_terminate(child);
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(status) => status.map_err(|source| ExecError::Wait {
            program: program.to_owned(),
            source,
        }),
        Err(_) => {
            send_kill(child);
            child.wait().await.map_err(|source| ExecError::Wait {
                program: program.to_owned(),
                source,
            })
        }
    }
}

#[cfg(unix)]
fn send_terminate(child: &mut Child) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let result = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .map_or(Err(nix::errno::Errno::ESRCH), |pid| {
            killpg(Pid::from_raw(pid), Signal::SIGTERM)
        });
    if result.is_err() {
        let _ = child.start_kill();
    }
}

#[cfg(not(unix))]
fn send_terminate(child: &mut Child) {
    let _ = child.start_kill();
}

#[cfg(unix)]
fn send_kill(child: &mut Child) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let result = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .map_or(Err(nix::errno::Errno::ESRCH), |pid| {
            killpg(Pid::from_raw(pid), Signal::SIGKILL)
        });
    if result.is_err() {
        let _ = child.start_kill();
    }
}

#[cfg(not(unix))]
fn send_kill(child: &mut Child) {
    let _ = child.start_kill();
}

fn natural_cause(status: &ExitStatus) -> TerminationCause {
    if status.code().is_some() {
        TerminationCause::Exited
    } else {
        TerminationCause::Signaled
    }
}

fn termination(cause: TerminationCause, status: ExitStatus) -> Termination {
    Termination {
        cause,
        exit_code: status.code(),
        signal: exit_signal(&status),
    }
}

#[cfg(unix)]
fn exit_signal(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;

    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}
