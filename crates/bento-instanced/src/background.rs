use crate::context::{DaemonContext, ServiceHandles};
use crate::guest;
use crate::host_export::listen_host_service;
use crate::server::InstanceServer;
use crate::state::Action;
use crate::StartupReporter;

pub async fn start(
    ctx: &DaemonContext,
    startup_reporter: &mut StartupReporter,
) -> eyre::Result<ServiceHandles> {
    let server = InstanceServer::new(
        ctx.machine.clone(),
        ctx.serial_console.clone(),
        ctx.services.host_services.clone(),
        ctx.store.clone(),
    );
    let control_socket = server.listen(
        &ctx.vm.file(bento_core::InstanceFile::InstancedSocket),
        ctx.shutdown.clone(),
    )?;

    let mut host_exports = tokio::task::JoinSet::new();
    for export in &ctx.host_socket_exports {
        let task = listen_host_service(
            ctx.machine.clone(),
            &export.host_path,
            export.name.clone(),
            export.port,
            ctx.shutdown.clone(),
        )?;
        host_exports.spawn(async move {
            match task.await {
                Ok(result) => result,
                Err(err) => Err(eyre::eyre!("host endpoint export task failed: {err}")),
            }
        });
    }

    ctx.store.dispatch(Action::vm_starting());
    ctx.machine.start().await?;
    ctx.store.dispatch(Action::vm_running());

    startup_reporter.report_started()?;

    let serial_log_path = ctx.vm.file(bento_core::InstanceFile::SerialLog);
    let serial_console_for_log = ctx.serial_console.clone();
    let serial_log = tokio::spawn(async move {
        if let Err(err) = serial_console_for_log
            .stream_to_file(&serial_log_path)
            .await
        {
            tracing::warn!(error = %err, path = %serial_log_path.display(), "serial log attachment failed");
        }
    });

    let guest_monitor = if ctx.expects_guest_agent {
        ctx.store.dispatch(Action::guest_starting());
        Some(guest::spawn_service_monitor(
            ctx.machine.clone(),
            ctx.services.host_services.clone(),
            ctx.store.clone(),
            ctx.shutdown.clone(),
        ))
    } else {
        ctx.store.dispatch(Action::set_services(Vec::new()));
        ctx.store.dispatch(Action::guest_running());
        None
    };

    tracing::info!(instance = %ctx.vm.name, "vmmon running");

    Ok(ServiceHandles {
        control_socket,
        guest_monitor,
        serial_log,
        host_exports,
    })
}
