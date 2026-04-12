use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::time::Duration;

use bento_core::{EndpointMode, EndpointSpec, RestartPolicy};
use bento_protocol::v1::{EndpointKind, EndpointStatus};
use nix::sys::socket::{
    sendmsg, socketpair, AddressFamily, ControlMessage, MsgFlags, SockFlag, SockType,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, LocalSet};
use tokio::time::Instant;

use crate::context::DaemonContext;
use crate::state::Action;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_RETRY_INITIAL: Duration = Duration::from_millis(200);
const CONNECT_RETRY_MAX: Duration = Duration::from_secs(5);
const STABLE_RUN_RESET: Duration = Duration::from_secs(30);
const FD_PASS_MAGIC: u32 = 0x4245_4e54;

pub(crate) fn start_endpoint_supervisor(ctx: DaemonContext) -> Option<JoinHandle<()>> {
    if ctx.spec.endpoints.is_empty() {
        return None;
    }

    for endpoint in &ctx.spec.endpoints {
        ctx.store
            .dispatch(Action::upsert_endpoint(base_status(endpoint)));
    }

    Some(tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build endpoint supervisor runtime");
        let local = LocalSet::new();

        runtime.block_on(local.run_until(async move {
            let mut handles = Vec::new();
            for endpoint in ctx.spec.endpoints.clone() {
                if !endpoint.lifecycle.autostart {
                    continue;
                }

                let endpoint_ctx = ctx.clone();
                handles.push(tokio::task::spawn_local(async move {
                    supervise_endpoint(endpoint_ctx, endpoint).await;
                }));
            }

            ctx.shutdown.cancelled().await;

            for handle in handles {
                if let Err(err) = handle.await {
                    tracing::error!(error = %err, "endpoint task failed during shutdown");
                }
            }
        }));
    }))
}

async fn supervise_endpoint(ctx: DaemonContext, endpoint: EndpointSpec) {
    let mut backoff = endpoint_backoff_initial(&endpoint);

    loop {
        if ctx.shutdown.is_cancelled() {
            set_endpoint_status(&ctx, &endpoint, false, "stopped", Vec::new());
            return;
        }

        let started_at = Instant::now();
        let outcome = match endpoint.mode {
            EndpointMode::Connect => run_connect_endpoint(&ctx, &endpoint).await,
            EndpointMode::Listen => run_listen_endpoint(&ctx, &endpoint).await,
        };

        let should_restart = match (&endpoint.lifecycle.restart, &outcome) {
            (_, EndpointOutcome::Shutdown) => false,
            (RestartPolicy::Never, _) => false,
            (RestartPolicy::OnFailure, EndpointOutcome::ExitedCleanly) => false,
            (RestartPolicy::OnFailure, EndpointOutcome::Failed(_)) => true,
            (RestartPolicy::Always, _) => true,
        };

        match &outcome {
            EndpointOutcome::Shutdown => {
                set_endpoint_status(&ctx, &endpoint, false, "stopped", Vec::new());
                return;
            }
            EndpointOutcome::ExitedCleanly => {
                set_endpoint_status(&ctx, &endpoint, false, "plugin exited", Vec::new());
            }
            EndpointOutcome::Failed(message) => {
                tracing::warn!(endpoint = %endpoint.name, error = %message, "endpoint failed");
                set_endpoint_status(&ctx, &endpoint, false, message, vec![message.clone()]);
            }
        }

        if !should_restart {
            return;
        }

        if started_at.elapsed() >= STABLE_RUN_RESET {
            backoff = endpoint_backoff_initial(&endpoint);
        }

        tokio::select! {
            _ = ctx.shutdown.cancelled() => {
                set_endpoint_status(&ctx, &endpoint, false, "stopped", Vec::new());
                return;
            }
            _ = tokio::time::sleep(backoff) => {}
        }

        backoff = std::cmp::min(backoff.saturating_mul(2), endpoint_backoff_max(&endpoint));
    }
}

async fn run_connect_endpoint(ctx: &DaemonContext, endpoint: &EndpointSpec) -> EndpointOutcome {
    let stream = match connect_with_retry(ctx, endpoint).await {
        Ok(stream) => stream,
        Err(outcome) => return outcome,
    };

    let fd = match stream.dup_fd() {
        Ok(fd) => fd,
        Err(err) => return EndpointOutcome::Failed(format!("duplicate stream fd: {err}")),
    };

    let startup = StartupMessage::new(endpoint, 3);
    let mut plugin = match spawn_plugin(endpoint, fd, &startup) {
        Ok(plugin) => plugin,
        Err(err) => return EndpointOutcome::Failed(format!("spawn plugin: {err}")),
    };

    let ready = wait_for_plugin_ready(ctx, endpoint, &mut plugin).await;
    if let Err(outcome) = ready {
        return outcome;
    }

    run_plugin_event_loop(ctx, endpoint, &mut plugin).await
}

async fn run_listen_endpoint(ctx: &DaemonContext, endpoint: &EndpointSpec) -> EndpointOutcome {
    let mut listener = match ctx.machine.listen_vsock(endpoint.port).await {
        Ok(listener) => listener,
        Err(err) => return EndpointOutcome::Failed(format!("listen vsock: {err}")),
    };

    let (control_parent, control_child) = match socketpair(
        AddressFamily::Unix,
        SockType::Stream,
        None,
        SockFlag::empty(),
    ) {
        Ok(pair) => pair,
        Err(err) => return EndpointOutcome::Failed(format!("create control socketpair: {err}")),
    };

    let startup = StartupMessage::new(endpoint, 3);
    let mut plugin = match spawn_plugin(endpoint, control_child, &startup) {
        Ok(plugin) => plugin,
        Err(err) => return EndpointOutcome::Failed(format!("spawn plugin: {err}")),
    };

    let ready = wait_for_plugin_ready(ctx, endpoint, &mut plugin).await;
    if let Err(outcome) = ready {
        return outcome;
    }

    let mut conn_id = 0_u64;
    loop {
        tokio::select! {
            _ = ctx.shutdown.cancelled() => {
                let _ = terminate_plugin(&mut plugin.child).await;
                return EndpointOutcome::Shutdown;
            }
            status = plugin.child.wait() => {
                return child_exit_outcome(status);
            }
            event = plugin.events.recv() => {
                if let Some(outcome) = handle_plugin_event(ctx, endpoint, event) {
                    if !matches!(outcome, EndpointOutcome::ExitedCleanly) {
                        let _ = terminate_plugin(&mut plugin.child).await;
                    }
                    return outcome;
                }
            }
            accept_result = listener.accept() => {
                let stream = match accept_result {
                    Ok(stream) => stream,
                    Err(err) => return EndpointOutcome::Failed(format!("accept vsock connection: {err}")),
                };

                let fd = match stream.dup_fd() {
                    Ok(fd) => fd,
                    Err(err) => return EndpointOutcome::Failed(format!("duplicate accepted fd: {err}")),
                };

                conn_id = conn_id.saturating_add(1);
                if let Err(err) = send_conn_fd_with_retry(ctx, endpoint, &control_parent, fd, conn_id).await {
                    let _ = terminate_plugin(&mut plugin.child).await;
                    return EndpointOutcome::Failed(err);
                }
            }
        }
    }
}

async fn connect_with_retry(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
) -> Result<bento_vmm::VsockStream, EndpointOutcome> {
    let mut backoff = CONNECT_RETRY_INITIAL;

    loop {
        let attempt =
            tokio::time::timeout(CONNECT_TIMEOUT, ctx.machine.connect_vsock(endpoint.port)).await;
        match attempt {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(err)) => {
                tracing::info!(endpoint = %endpoint.name, error = %err, "endpoint connect retrying");
            }
            Err(_) => {
                tracing::info!(endpoint = %endpoint.name, timeout = ?CONNECT_TIMEOUT, "endpoint connect timed out, retrying");
            }
        }

        tokio::select! {
            _ = ctx.shutdown.cancelled() => return Err(EndpointOutcome::Shutdown),
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = std::cmp::min(backoff.saturating_mul(2), CONNECT_RETRY_MAX);
    }
}

async fn wait_for_plugin_ready(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
    plugin: &mut RunningPlugin,
) -> Result<(), EndpointOutcome> {
    let timeout = Duration::from_millis(endpoint.lifecycle.startup_timeout_ms);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = ctx.shutdown.cancelled() => {
                let _ = terminate_plugin(&mut plugin.child).await;
                return Err(EndpointOutcome::Shutdown);
            }
            _ = &mut deadline => {
                let _ = terminate_plugin(&mut plugin.child).await;
                return Err(EndpointOutcome::Failed(format!("plugin did not become ready within {timeout:?}")));
            }
            status = plugin.child.wait() => {
                return Err(child_exit_outcome(status));
            }
            event = plugin.events.recv() => {
                match event {
                    Some(PluginEvent::Ready) => return Ok(()),
                    Some(PluginEvent::Failed(message)) => {
                        let _ = terminate_plugin(&mut plugin.child).await;
                        return Err(EndpointOutcome::Failed(message));
                    }
                    Some(PluginEvent::EndpointStatus { active, summary, problems }) => {
                        set_endpoint_status(ctx, endpoint, active, &summary, problems);
                    }
                    Some(PluginEvent::Healthy) => {
                        set_endpoint_status(ctx, endpoint, false, "healthy", Vec::new());
                    }
                    Some(PluginEvent::Degraded(message)) => {
                        set_endpoint_status(ctx, endpoint, false, &message, vec![message.clone()]);
                    }
                    Some(PluginEvent::Stopping(message)) => {
                        set_endpoint_status(ctx, endpoint, false, &message, vec![message.clone()]);
                    }
                    None => return Err(EndpointOutcome::Failed("plugin stdout closed before ready".to_string())),
                }
            }
        }
    }
}

async fn run_plugin_event_loop(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
    plugin: &mut RunningPlugin,
) -> EndpointOutcome {
    loop {
        tokio::select! {
            _ = ctx.shutdown.cancelled() => {
                let _ = terminate_plugin(&mut plugin.child).await;
                return EndpointOutcome::Shutdown;
            }
            status = plugin.child.wait() => {
                return child_exit_outcome(status);
            }
            event = plugin.events.recv() => {
                if let Some(outcome) = handle_plugin_event(ctx, endpoint, event) {
                    if !matches!(outcome, EndpointOutcome::ExitedCleanly) {
                        let _ = terminate_plugin(&mut plugin.child).await;
                    }
                    return outcome;
                }
            }
        }
    }
}

fn handle_plugin_event(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
    event: Option<PluginEvent>,
) -> Option<EndpointOutcome> {
    match event {
        Some(PluginEvent::Ready) => None,
        Some(PluginEvent::Failed(message)) => Some(EndpointOutcome::Failed(message)),
        Some(PluginEvent::EndpointStatus {
            active,
            summary,
            problems,
        }) => {
            set_endpoint_status(ctx, endpoint, active, &summary, problems);
            None
        }
        Some(PluginEvent::Healthy) => {
            set_endpoint_status(ctx, endpoint, false, "healthy", Vec::new());
            None
        }
        Some(PluginEvent::Degraded(message)) => {
            set_endpoint_status(ctx, endpoint, false, &message, vec![message.clone()]);
            None
        }
        Some(PluginEvent::Stopping(message)) => {
            set_endpoint_status(ctx, endpoint, false, &message, vec![message.clone()]);
            None
        }
        None => Some(EndpointOutcome::Failed(
            "plugin stdout closed unexpectedly".to_string(),
        )),
    }
}

fn spawn_plugin(
    endpoint: &EndpointSpec,
    fd3: OwnedFd,
    startup: &StartupMessage,
) -> io::Result<RunningPlugin> {
    let raw_fd3 = fd3.as_raw_fd();
    let mut command = Command::new(&endpoint.plugin.command);
    command
        .args(&endpoint.plugin.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);

    if let Some(working_dir) = &endpoint.plugin.working_dir {
        command.current_dir(working_dir);
    }
    for (key, value) in &endpoint.plugin.env {
        command.env(key, value);
    }

    unsafe {
        command.as_std_mut().pre_exec(move || {
            if libc::dup2(raw_fd3, 3) == -1 {
                return Err(io::Error::last_os_error());
            }

            let flags = libc::fcntl(3, libc::F_GETFD);
            if flags == -1 {
                return Err(io::Error::last_os_error());
            }

            if libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(io::Error::last_os_error());
            }

            if raw_fd3 != 3 && libc::close(raw_fd3) == -1 {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        });
    }

    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "plugin stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "plugin stdout unavailable"))?;

    let payload = serde_json::to_vec(startup).map_err(io::Error::other)?;
    let (tx, rx) = mpsc::unbounded_channel();
    spawn_stdout_reader(stdout, tx);

    tokio::spawn(async move {
        if let Err(err) = stdin.write_all(&payload).await {
            tracing::warn!(error = %err, "failed to write plugin startup payload");
            return;
        }
        if let Err(err) = stdin.write_all(b"\n").await {
            tracing::warn!(error = %err, "failed to terminate plugin startup payload");
        }
    });

    Ok(RunningPlugin { child, events: rx })
}

fn spawn_stdout_reader(stdout: ChildStdout, tx: mpsc::UnboundedSender<PluginEvent>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match serde_json::from_str::<PluginStdoutEvent>(&line) {
                    Ok(event) => {
                        if tx.send(event.into()).is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(PluginEvent::Failed(format!(
                            "invalid plugin stdout event: {err}"
                        )));
                        return;
                    }
                },
                Ok(None) => return,
                Err(err) => {
                    let _ = tx.send(PluginEvent::Failed(format!("read plugin stdout: {err}")));
                    return;
                }
            }
        }
    });
}

async fn terminate_plugin(child: &mut Child) -> io::Result<()> {
    match child.kill().await {
        Ok(()) => {
            let _ = child.wait().await;
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(err) => Err(err),
    }
}

async fn send_conn_fd_with_retry(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
    control: &OwnedFd,
    conn_fd: OwnedFd,
    conn_id: u64,
) -> Result<(), String> {
    let mut backoff = endpoint_backoff_initial(endpoint);
    loop {
        match send_conn_fd(control, &conn_fd, conn_id) {
            Ok(()) => return Ok(()),
            Err(err) if err.raw_os_error() == Some(libc::ETOOMANYREFS) => {
                tracing::warn!(endpoint = %endpoint.name, error = %err, "endpoint fd passing hit backpressure");
            }
            Err(err) => return Err(format!("send connection fd: {err}")),
        }

        tokio::select! {
            _ = ctx.shutdown.cancelled() => return Err("shutdown requested".to_string()),
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = std::cmp::min(backoff.saturating_mul(2), endpoint_backoff_max(endpoint));
    }
}

fn send_conn_fd(control: &OwnedFd, conn_fd: &OwnedFd, conn_id: u64) -> io::Result<()> {
    let payload = BentoFdPassV1 {
        magic: FD_PASS_MAGIC,
        flags: 0,
        conn_id,
    };
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&payload as *const BentoFdPassV1).cast::<u8>(),
            std::mem::size_of::<BentoFdPassV1>(),
        )
    };
    let iov = [std::io::IoSlice::new(bytes)];
    let fds = [conn_fd.as_raw_fd()];
    let cmsg = [ControlMessage::ScmRights(&fds)];

    sendmsg::<()>(control.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)
        .map_err(io::Error::other)?;
    Ok(())
}

fn base_status(endpoint: &EndpointSpec) -> EndpointStatus {
    EndpointStatus {
        name: endpoint.name.clone(),
        kind: endpoint_kind(endpoint.mode) as i32,
        port: endpoint.port,
        active: false,
        summary: String::new(),
        problems: Vec::new(),
    }
}

fn set_endpoint_status(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
    active: bool,
    summary: impl Into<String>,
    problems: Vec<String>,
) {
    let mut status = base_status(endpoint);
    status.active = active;
    status.summary = summary.into();
    status.problems = problems;
    ctx.store.dispatch(Action::upsert_endpoint(status));
}

fn endpoint_kind(mode: EndpointMode) -> EndpointKind {
    match mode {
        EndpointMode::Connect => EndpointKind::VsockConnect,
        EndpointMode::Listen => EndpointKind::VsockListen,
    }
}

fn endpoint_backoff_initial(endpoint: &EndpointSpec) -> Duration {
    Duration::from_millis(endpoint.lifecycle.backoff_ms.initial)
}

fn endpoint_backoff_max(endpoint: &EndpointSpec) -> Duration {
    Duration::from_millis(endpoint.lifecycle.backoff_ms.max)
}

fn child_exit_outcome(status: io::Result<std::process::ExitStatus>) -> EndpointOutcome {
    match status {
        Ok(exit_status) if exit_status.success() => EndpointOutcome::ExitedCleanly,
        Ok(exit_status) => {
            EndpointOutcome::Failed(format!("plugin exited with status {exit_status}"))
        }
        Err(err) => EndpointOutcome::Failed(format!("wait for plugin exit: {err}")),
    }
}

#[derive(Debug)]
enum EndpointOutcome {
    Shutdown,
    ExitedCleanly,
    Failed(String),
}

struct RunningPlugin {
    child: Child,
    events: mpsc::UnboundedReceiver<PluginEvent>,
}

#[derive(Debug, serde::Serialize)]
struct StartupMessage {
    api_version: u32,
    endpoint: String,
    mode: EndpointMode,
    port: u32,
    fd: i32,
}

impl StartupMessage {
    fn new(endpoint: &EndpointSpec, fd: i32) -> Self {
        Self {
            api_version: 1,
            endpoint: endpoint.name.clone(),
            mode: endpoint.mode,
            port: endpoint.port,
            fd,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum PluginStdoutEvent {
    Ready,
    Failed {
        message: String,
    },
    Healthy,
    Degraded {
        message: String,
    },
    Stopping {
        message: String,
    },
    EndpointStatus {
        active: bool,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        problems: Vec<String>,
    },
}

impl From<PluginStdoutEvent> for PluginEvent {
    fn from(value: PluginStdoutEvent) -> Self {
        match value {
            PluginStdoutEvent::Ready => Self::Ready,
            PluginStdoutEvent::Failed { message } => Self::Failed(message),
            PluginStdoutEvent::Healthy => Self::Healthy,
            PluginStdoutEvent::Degraded { message } => Self::Degraded(message),
            PluginStdoutEvent::Stopping { message } => Self::Stopping(message),
            PluginStdoutEvent::EndpointStatus {
                active,
                summary,
                problems,
            } => Self::EndpointStatus {
                active,
                summary,
                problems,
            },
        }
    }
}

#[derive(Debug)]
enum PluginEvent {
    Ready,
    Failed(String),
    Healthy,
    Degraded(String),
    Stopping(String),
    EndpointStatus {
        active: bool,
        summary: String,
        problems: Vec<String>,
    },
}

#[repr(C)]
struct BentoFdPassV1 {
    magic: u32,
    flags: u32,
    conn_id: u64,
}

#[cfg(test)]
mod tests {
    use super::PluginStdoutEvent;

    #[test]
    fn parse_endpoint_status_event() {
        let raw =
            r#"{"event":"endpoint_status","active":true,"summary":"ready","problems":["none"]}"#;
        let event: PluginStdoutEvent = serde_json::from_str(raw).expect("event should parse");
        match event {
            PluginStdoutEvent::EndpointStatus {
                active,
                summary,
                problems,
            } => {
                assert!(active);
                assert_eq!(summary, "ready");
                assert_eq!(problems, vec!["none"]);
            }
            _ => panic!("expected endpoint_status event"),
        }
    }
}
