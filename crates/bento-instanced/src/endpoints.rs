use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bento_core::{EndpointMode, EndpointSpec, RestartPolicy};
use bento_protocol::v1::{EndpointKind, EndpointStatus};
use nix::errno::Errno;
use nix::sys::socket::{
    recvmsg, sendmsg, socketpair, AddressFamily, ControlMessage, MsgFlags, SockFlag, SockType,
};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::context::DaemonContext;
use crate::state::Action;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const STABLE_RUN_RESET: Duration = Duration::from_secs(30);
const CONTROL_MAGIC: u32 = 0x4252_4b52;
const CONTROL_MESSAGE_MAX_BYTES: usize = 256;

pub(crate) fn start_endpoint_supervisor(
    ctx: DaemonContext,
    instance_dir: PathBuf,
) -> Option<JoinHandle<()>> {
    if ctx.spec.endpoints.is_empty() {
        return None;
    }

    for endpoint in &ctx.spec.endpoints {
        ctx.store
            .dispatch(Action::upsert_endpoint(base_status(endpoint)));
    }

    Some(tokio::spawn(async move {
        let mut handles = Vec::new();
        for endpoint in ctx.spec.endpoints.clone() {
            if !endpoint.lifecycle.autostart {
                continue;
            }

            let endpoint_ctx = ctx.clone();
            let endpoint_instance_dir = instance_dir.clone();
            handles.push(tokio::spawn(async move {
                supervise_endpoint(endpoint_ctx, endpoint_instance_dir, endpoint).await;
            }));
        }

        ctx.shutdown.cancelled().await;

        for handle in handles {
            if let Err(err) = handle.await {
                tracing::error!(error = %err, "endpoint task failed during shutdown");
            }
        }
    }))
}

async fn supervise_endpoint(ctx: DaemonContext, instance_dir: PathBuf, endpoint: EndpointSpec) {
    let mut backoff = endpoint_backoff_initial(&endpoint);

    tracing::info!(
        endpoint = %endpoint.name,
        mode = %endpoint_mode_name(endpoint.mode),
        port = endpoint.port,
        autostart = endpoint.lifecycle.autostart,
        restart_policy = %restart_policy_name(endpoint.lifecycle.restart),
        "starting endpoint supervision"
    );

    loop {
        if ctx.shutdown.is_cancelled() {
            set_endpoint_status(&ctx, &endpoint, false, "stopped", Vec::new());
            return;
        }

        let started_at = Instant::now();
        let outcome = match endpoint.mode {
            EndpointMode::Connect => run_connect_endpoint(&ctx, &instance_dir, &endpoint).await,
            EndpointMode::Listen => run_listen_endpoint(&ctx, &instance_dir, &endpoint).await,
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
                tracing::info!(
                    endpoint = %endpoint.name,
                    mode = %endpoint_mode_name(endpoint.mode),
                    port = endpoint.port,
                    "endpoint plugin exited cleanly"
                );
                set_endpoint_status(&ctx, &endpoint, false, "plugin exited", Vec::new());
            }
            EndpointOutcome::Failed(message) => {
                tracing::error!(
                    endpoint = %endpoint.name,
                    mode = %endpoint_mode_name(endpoint.mode),
                    port = endpoint.port,
                    error = %message,
                    "endpoint failed"
                );
                set_endpoint_status(&ctx, &endpoint, false, message, vec![message.clone()]);
            }
        }

        if !should_restart {
            tracing::warn!(
                endpoint = %endpoint.name,
                mode = %endpoint_mode_name(endpoint.mode),
                port = endpoint.port,
                restart_policy = %restart_policy_name(endpoint.lifecycle.restart),
                "endpoint plugin will not be restarted"
            );
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

        tracing::warn!(
            endpoint = %endpoint.name,
            mode = %endpoint_mode_name(endpoint.mode),
            port = endpoint.port,
            restart_policy = %restart_policy_name(endpoint.lifecycle.restart),
            backoff = ?backoff,
            "restarting endpoint plugin after failure"
        );

        backoff = std::cmp::min(backoff.saturating_mul(2), endpoint_backoff_max(&endpoint));
    }
}

async fn run_connect_endpoint(
    ctx: &DaemonContext,
    instance_dir: &Path,
    endpoint: &EndpointSpec,
) -> EndpointOutcome {
    let (control_parent, mut plugin) = match start_endpoint_plugin(instance_dir, endpoint) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };

    let broker = tokio::spawn(run_connect_broker(
        ctx.clone(),
        endpoint.clone(),
        control_parent,
    ));

    let ready = wait_for_plugin_ready(ctx, endpoint, &mut plugin).await;
    if let Err(outcome) = ready {
        broker.abort();
        return outcome;
    }

    let outcome = run_plugin_event_loop(ctx, endpoint, &mut plugin, Some(broker)).await;
    if !matches!(outcome, EndpointOutcome::Shutdown) {
        let _ = terminate_plugin(&mut plugin.child).await;
    }
    outcome
}

async fn run_listen_endpoint(
    ctx: &DaemonContext,
    instance_dir: &Path,
    endpoint: &EndpointSpec,
) -> EndpointOutcome {
    let listener = match ctx.machine.listen_vsock(endpoint.port).await {
        Ok(listener) => listener,
        Err(err) => return EndpointOutcome::Failed(format!("listen vsock: {err}")),
    };

    let (control_parent, mut plugin) = match start_endpoint_plugin(instance_dir, endpoint) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };

    let ready = wait_for_plugin_ready(ctx, endpoint, &mut plugin).await;
    if let Err(outcome) = ready {
        return outcome;
    }

    let dispatcher = tokio::spawn(run_listen_dispatch(
        ctx.clone(),
        endpoint.clone(),
        listener,
        control_parent,
    ));

    let outcome = run_plugin_event_loop(ctx, endpoint, &mut plugin, Some(dispatcher)).await;
    if !matches!(outcome, EndpointOutcome::Shutdown) {
        let _ = terminate_plugin(&mut plugin.child).await;
    }
    outcome
}

fn start_endpoint_plugin(
    instance_dir: &Path,
    endpoint: &EndpointSpec,
) -> Result<(OwnedFd, RunningPlugin), EndpointOutcome> {
    let (control_parent, control_child) = match socketpair(
        AddressFamily::Unix,
        SockType::Datagram,
        None,
        SockFlag::empty(),
    ) {
        Ok(pair) => pair,
        Err(err) => {
            return Err(EndpointOutcome::Failed(format!(
                "create control socketpair: {err}"
            )))
        }
    };

    let runtime_dir = instance_dir.to_path_buf();
    if let Err(err) = std::fs::create_dir_all(&runtime_dir) {
        return Err(EndpointOutcome::Failed(format!(
            "create endpoint runtime dir {}: {err}",
            runtime_dir.display()
        )));
    }

    let startup = StartupMessage::new(endpoint, runtime_dir, 3);
    tracing::info!(
        endpoint = %endpoint.name,
        mode = %endpoint_mode_name(endpoint.mode),
        port = endpoint.port,
        command = %endpoint.plugin.command.display(),
        args = ?endpoint.plugin.args,
        working_dir = ?endpoint.plugin.working_dir,
        "starting endpoint plugin"
    );
    let plugin = match spawn_plugin(endpoint, control_child, &startup) {
        Ok(plugin) => plugin,
        Err(err) => return Err(EndpointOutcome::Failed(format!("spawn plugin: {err}"))),
    };

    Ok((control_parent, plugin))
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
                    Some(PluginEvent::Ready) => {
                        tracing::info!(
                            endpoint = %endpoint.name,
                            mode = %endpoint_mode_name(endpoint.mode),
                            port = endpoint.port,
                            "endpoint plugin ready"
                        );
                        return Ok(());
                    }
                    Some(PluginEvent::Failed(message)) => {
                        tracing::error!(
                            endpoint = %endpoint.name,
                            mode = %endpoint_mode_name(endpoint.mode),
                            port = endpoint.port,
                            plugin_message = %message,
                            "endpoint plugin reported failure"
                        );
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
    broker: Option<JoinHandle<Result<(), String>>>,
) -> EndpointOutcome {
    let mut broker = broker;
    loop {
        tokio::select! {
            _ = ctx.shutdown.cancelled() => {
                stop_control_task(&mut broker).await;
                let _ = terminate_plugin(&mut plugin.child).await;
                return EndpointOutcome::Shutdown;
            }
            status = plugin.child.wait() => {
                stop_control_task(&mut broker).await;
                return child_exit_outcome(status);
            }
            event = plugin.events.recv() => {
                if let Some(outcome) = handle_plugin_event(ctx, endpoint, event) {
                    stop_control_task(&mut broker).await;
                    if !matches!(outcome, EndpointOutcome::ExitedCleanly) {
                        let _ = terminate_plugin(&mut plugin.child).await;
                    }
                    return outcome;
                }
            }
            result = await_broker(&mut broker), if broker.is_some() => {
                if !matches!(result, Ok(())) {
                    let _ = terminate_plugin(&mut plugin.child).await;
                    return EndpointOutcome::Failed(result.err().unwrap());
                }
            }
        }
    }
}

async fn stop_control_task(broker: &mut Option<JoinHandle<Result<(), String>>>) {
    let Some(handle) = broker.take() else {
        return;
    };

    handle.abort();
    match handle.await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::debug!(error = %err, "endpoint control task returned error during shutdown");
        }
        Err(err) if err.is_cancelled() => {}
        Err(err) => {
            tracing::debug!(error = %err, "endpoint control task join failed during shutdown");
        }
    }
}

async fn await_broker(broker: &mut Option<JoinHandle<Result<(), String>>>) -> Result<(), String> {
    let handle = broker
        .take()
        .expect("broker handle should exist when awaited");
    match handle.await {
        Ok(result) => result,
        Err(err) if err.is_cancelled() => Ok(()),
        Err(err) => Err(format!("broker task failed: {err}")),
    }
}

fn handle_plugin_event(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
    event: Option<PluginEvent>,
) -> Option<EndpointOutcome> {
    match event {
        Some(PluginEvent::Ready) => None,
        Some(PluginEvent::Failed(message)) => {
            tracing::error!(
                endpoint = %endpoint.name,
                mode = %endpoint_mode_name(endpoint.mode),
                port = endpoint.port,
                plugin_message = %message,
                "endpoint plugin reported failure"
            );
            Some(EndpointOutcome::Failed(message))
        }
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
    tracing::info!(
        endpoint = %endpoint.name,
        mode = %endpoint_mode_name(endpoint.mode),
        port = endpoint.port,
        pid = ?child.id(),
        command = %endpoint.plugin.command.display(),
        "spawned endpoint plugin"
    );
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

async fn send_incoming_conn_with_retry(
    ctx: &DaemonContext,
    endpoint: &EndpointSpec,
    control: &OwnedFd,
    conn_fd: OwnedFd,
    conn_id: u64,
) -> Result<(), String> {
    let mut backoff = endpoint_backoff_initial(endpoint);
    loop {
        match send_control_message(
            control,
            &ControlMessageKind::ListenIncoming { conn_id },
            Some(&conn_fd),
        ) {
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

async fn run_connect_broker(
    ctx: DaemonContext,
    endpoint: EndpointSpec,
    control: OwnedFd,
) -> Result<(), String> {
    let control = Arc::new(
        BrokerControlSocket::new(control).map_err(|err| format!("wrap broker socket: {err}"))?,
    );

    loop {
        let message = match control.recv_message().await {
            Ok(message) => message,
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(format!("read broker request: {err}")),
        };

        match message {
            ControlMessageKind::ConnectOpen { request_id } => {
                let result =
                    tokio::time::timeout(CONNECT_TIMEOUT, ctx.machine.connect_vsock(endpoint.port))
                        .await;
                match result {
                    Ok(Ok(stream)) => {
                        let fd = stream
                            .dup_fd()
                            .map_err(|err| format!("duplicate stream fd: {err}"))?;
                        control
                            .send_message(
                                &ControlMessageKind::ConnectOpenOk { request_id },
                                Some(&fd),
                            )
                            .await
                            .map_err(|err| format!("send connect_open_ok: {err}"))?;
                    }
                    Ok(Err(err)) => {
                        tracing::info!(endpoint = %endpoint.name, error = %err, "broker connect request failed");
                        control
                            .send_message(
                                &ControlMessageKind::ConnectOpenErr {
                                    request_id,
                                    retryable: true,
                                    message: format!("connect_vsock: {err}"),
                                },
                                None,
                            )
                            .await
                            .map_err(|err| format!("send connect_open_err: {err}"))?;
                    }
                    Err(_) => {
                        tracing::info!(endpoint = %endpoint.name, timeout = ?CONNECT_TIMEOUT, "broker connect request timed out");
                        control
                            .send_message(
                                &ControlMessageKind::ConnectOpenErr {
                                    request_id,
                                    retryable: true,
                                    message: format!(
                                        "connect_vsock timed out after {CONNECT_TIMEOUT:?}"
                                    ),
                                },
                                None,
                            )
                            .await
                            .map_err(|err| format!("send connect_open_err: {err}"))?;
                    }
                }
            }
            other => {
                return Err(format!(
                    "unexpected control message for connect broker: {}",
                    control_message_name(&other)
                ));
            }
        }
    }
}

async fn run_listen_dispatch(
    ctx: DaemonContext,
    endpoint: EndpointSpec,
    mut listener: bento_vmm::VsockListener,
    control: OwnedFd,
) -> Result<(), String> {
    let mut conn_id = 0_u64;
    loop {
        tokio::select! {
            _ = ctx.shutdown.cancelled() => return Ok(()),
            accept_result = listener.accept() => {
                let stream = accept_result
                    .map_err(|err| format!("accept vsock connection: {err}"))?;

                let fd = stream
                    .dup_fd()
                    .map_err(|err| format!("duplicate accepted fd: {err}"))?;

                conn_id = conn_id.saturating_add(1);
                tracing::debug!(
                    endpoint = %endpoint.name,
                    mode = %endpoint_mode_name(endpoint.mode),
                    port = endpoint.port,
                    conn_id,
                    "accepted endpoint connection"
                );
                send_incoming_conn_with_retry(&ctx, &endpoint, &control, fd, conn_id).await?;
                tracing::debug!(
                    endpoint = %endpoint.name,
                    mode = %endpoint_mode_name(endpoint.mode),
                    port = endpoint.port,
                    conn_id,
                    "handed endpoint connection to plugin"
                );
            }
        }
    }
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
    transport: PluginTransport,
    runtime_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<serde_json::Value>,
    fd: i32,
}

impl StartupMessage {
    fn new(endpoint: &EndpointSpec, runtime_dir: PathBuf, fd: i32) -> Self {
        Self {
            api_version: 1,
            endpoint: endpoint.name.clone(),
            mode: endpoint.mode,
            port: endpoint.port,
            transport: PluginTransport::for_mode(endpoint.mode),
            runtime_dir: runtime_dir.to_string_lossy().into_owned(),
            config: endpoint.plugin.config.clone(),
            fd,
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum PluginTransport {
    BrokeredConnect,
    ListenAccept,
}

impl PluginTransport {
    fn for_mode(mode: EndpointMode) -> Self {
        match mode {
            EndpointMode::Connect => Self::BrokeredConnect,
            EndpointMode::Listen => Self::ListenAccept,
        }
    }
}

enum ControlMessageKind {
    ConnectOpen {
        request_id: u64,
    },
    ConnectOpenOk {
        request_id: u64,
    },
    ConnectOpenErr {
        request_id: u64,
        retryable: bool,
        message: String,
    },
    ListenIncoming {
        conn_id: u64,
    },
}

struct BrokerControlSocket {
    inner: AsyncFd<OwnedFd>,
}

impl BrokerControlSocket {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        set_nonblocking(fd.as_raw_fd())?;
        Ok(Self {
            inner: AsyncFd::new(fd)?,
        })
    }

    async fn recv_message(&self) -> io::Result<ControlMessageKind> {
        let payload = loop {
            let mut guard = self.inner.readable().await?;
            match guard.try_io(|inner| recv_broker_frame(inner.get_ref().as_raw_fd())) {
                Ok(result) => break result?,
                Err(_would_block) => continue,
            }
        };

        ControlMessageKind::from_bytes(&payload)
    }

    async fn send_message(
        &self,
        message: &ControlMessageKind,
        fd: Option<&OwnedFd>,
    ) -> io::Result<()> {
        let payload = message.to_bytes()?;

        loop {
            let mut guard = self.inner.writable().await?;
            match guard.try_io(|inner| send_broker_frame(inner.get_ref().as_raw_fd(), &payload, fd))
            {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }
}

fn send_broker_frame(control: RawFd, payload: &[u8], fd: Option<&OwnedFd>) -> io::Result<()> {
    let iov = [std::io::IoSlice::new(payload)];
    let sent = match fd {
        Some(fd) => {
            let fds = [fd.as_raw_fd()];
            let cmsg = [ControlMessage::ScmRights(&fds)];
            sendmsg::<()>(control, &iov, &cmsg, MsgFlags::empty(), None)
        }
        None => sendmsg::<()>(control, &iov, &[], MsgFlags::empty(), None),
    }
    .map_err(nix_errno_to_io_error)?;

    if sent != payload.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            format!("short broker write: sent {sent} of {} bytes", payload.len()),
        ));
    }

    Ok(())
}

fn send_control_message(
    control: &OwnedFd,
    message: &ControlMessageKind,
    fd: Option<&OwnedFd>,
) -> io::Result<()> {
    let payload = message.to_bytes()?;
    send_broker_frame(control.as_raw_fd(), &payload, fd)
}

fn recv_broker_frame(control: RawFd) -> io::Result<Vec<u8>> {
    let mut payload = vec![0_u8; std::mem::size_of::<ControlMessageFrameV1>()];
    let bytes = {
        let mut iov = [std::io::IoSliceMut::new(&mut payload)];
        recvmsg::<()>(control, &mut iov, None, MsgFlags::empty())
            .map_err(nix_errno_to_io_error)?
            .bytes
    };
    if bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "broker socket closed",
        ));
    }

    payload.truncate(bytes);
    Ok(payload)
}

fn control_message_name(message: &ControlMessageKind) -> &'static str {
    match message {
        ControlMessageKind::ConnectOpen { .. } => "connect_open",
        ControlMessageKind::ConnectOpenOk { .. } => "connect_open_ok",
        ControlMessageKind::ConnectOpenErr { .. } => "connect_open_err",
        ControlMessageKind::ListenIncoming { .. } => "listen_incoming",
    }
}

impl ControlMessageKind {
    fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != std::mem::size_of::<ControlMessageFrameV1>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected control message size {}", bytes.len()),
            ));
        }

        let magic = u32::from_ne_bytes(bytes[0..4].try_into().expect("slice is four bytes"));
        if magic != CONTROL_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected control magic {magic:#x}"),
            ));
        }

        let kind = u32::from_ne_bytes(bytes[4..8].try_into().expect("slice is four bytes"));
        let id = u64::from_ne_bytes(bytes[8..16].try_into().expect("slice is eight bytes"));
        let flags = u32::from_ne_bytes(bytes[16..20].try_into().expect("slice is four bytes"));
        let message_len =
            u32::from_ne_bytes(bytes[20..24].try_into().expect("slice is four bytes")) as usize;

        if message_len > CONTROL_MESSAGE_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control message exceeded max size",
            ));
        }

        let message = String::from_utf8(bytes[24..24 + message_len].to_vec()).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("decode control message: {err}"),
            )
        })?;

        match kind {
            1 => Ok(Self::ConnectOpen { request_id: id }),
            2 => Ok(Self::ConnectOpenOk { request_id: id }),
            3 => Ok(Self::ConnectOpenErr {
                request_id: id,
                retryable: flags != 0,
                message,
            }),
            4 => Ok(Self::ListenIncoming { conn_id: id }),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown control message kind {kind}"),
            )),
        }
    }

    fn to_bytes(&self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(std::mem::size_of::<ControlMessageFrameV1>());

        match self {
            Self::ConnectOpen { request_id } => {
                bytes.extend_from_slice(&CONTROL_MAGIC.to_ne_bytes());
                bytes.extend_from_slice(&1_u32.to_ne_bytes());
                bytes.extend_from_slice(&request_id.to_ne_bytes());
                bytes.extend_from_slice(&0_u32.to_ne_bytes());
                bytes.extend_from_slice(&0_u32.to_ne_bytes());
            }
            Self::ConnectOpenOk { request_id } => {
                bytes.extend_from_slice(&CONTROL_MAGIC.to_ne_bytes());
                bytes.extend_from_slice(&2_u32.to_ne_bytes());
                bytes.extend_from_slice(&request_id.to_ne_bytes());
                bytes.extend_from_slice(&0_u32.to_ne_bytes());
                bytes.extend_from_slice(&0_u32.to_ne_bytes());
            }
            Self::ConnectOpenErr {
                request_id,
                retryable,
                message,
            } => {
                let message_bytes = message.as_bytes();
                if message_bytes.len() > CONTROL_MESSAGE_MAX_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "control error message exceeds max size",
                    ));
                }

                bytes.extend_from_slice(&CONTROL_MAGIC.to_ne_bytes());
                bytes.extend_from_slice(&3_u32.to_ne_bytes());
                bytes.extend_from_slice(&request_id.to_ne_bytes());
                bytes.extend_from_slice(&u32::from(*retryable).to_ne_bytes());
                bytes.extend_from_slice(&(message_bytes.len() as u32).to_ne_bytes());
                bytes.extend_from_slice(message_bytes);
            }
            Self::ListenIncoming { conn_id } => {
                bytes.extend_from_slice(&CONTROL_MAGIC.to_ne_bytes());
                bytes.extend_from_slice(&4_u32.to_ne_bytes());
                bytes.extend_from_slice(&conn_id.to_ne_bytes());
                bytes.extend_from_slice(&0_u32.to_ne_bytes());
                bytes.extend_from_slice(&0_u32.to_ne_bytes());
            }
        }
        bytes.resize(std::mem::size_of::<ControlMessageFrameV1>(), 0);

        Ok(bytes)
    }
}

#[repr(C)]
struct ControlMessageFrameV1 {
    magic: u32,
    kind: u32,
    id: u64,
    flags: u32,
    message_len: u32,
    message: [u8; CONTROL_MESSAGE_MAX_BYTES],
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn nix_errno_to_io_error(err: Errno) -> io::Error {
    io::Error::from_raw_os_error(err as i32)
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

fn endpoint_mode_name(mode: EndpointMode) -> &'static str {
    match mode {
        EndpointMode::Connect => "connect",
        EndpointMode::Listen => "listen",
    }
}

fn restart_policy_name(policy: RestartPolicy) -> &'static str {
    match policy {
        RestartPolicy::Never => "never",
        RestartPolicy::OnFailure => "on_failure",
        RestartPolicy::Always => "always",
    }
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
