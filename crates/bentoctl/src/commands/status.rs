use clap::Args;
use std::fmt::{Display, Formatter};

use bento_protocol::instance::v1::LifecycleState;
use bento_runtime::instance::{InstanceFile, InstanceStatus};
use bento_runtime::instance_store::InstanceStore;

use crate::service_readiness;

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub async fn run(&self, store: &InstanceStore) -> eyre::Result<()> {
        let inst = store.inspect(&self.name)?;

        println!("name: {}", inst.name);
        println!("process: {:?}", inst.status());

        if inst.status() != InstanceStatus::Running {
            println!("ready: no");
            return Ok(());
        }

        let status =
            service_readiness::get_instance_status(&inst.file(InstanceFile::InstancedSocket))
                .await?;

        println!("vm: {}", lifecycle_label(status.vm_state));
        println!("guest: {}", lifecycle_label(status.guest_state));
        println!("ready: {}", if status.ready { "yes" } else { "no" });
        if !status.summary.is_empty() {
            println!("summary: {}", status.summary);
        }

        if !status.extensions.is_empty() {
            println!("extensions:");
            for extension in status.extensions {
                println!(
                    "  - {} enabled={} startup_required={} configured={} running={}",
                    extension.name,
                    extension.enabled,
                    extension.startup_required,
                    extension.configured,
                    extension.running,
                );
                if !extension.summary.is_empty() {
                    println!("    summary: {}", extension.summary);
                }
                for problem in extension.problems {
                    println!("    problem: {}", problem);
                }
            }
        }

        if !status.host_sockets.is_empty() {
            println!("host sockets:");
            for socket in status.host_sockets {
                println!("  - {} => {}", socket.name, socket.path);
            }
        }

        if !status.port_forwards.is_empty() {
            println!("port forwards:");
            for forward in status.port_forwards {
                println!(
                    "  - guest:{} host:{} active={}",
                    forward.guest_port, forward.host_port, forward.active
                );
                if !forward.message.is_empty() {
                    println!("    message: {}", forward.message);
                }
            }
        }

        Ok(())
    }
}

fn lifecycle_label(raw: i32) -> &'static str {
    match LifecycleState::try_from(raw).unwrap_or(LifecycleState::Unspecified) {
        LifecycleState::Unspecified => "unspecified",
        LifecycleState::Starting => "starting",
        LifecycleState::Running => "running",
        LifecycleState::Stopping => "stopping",
        LifecycleState::Stopped => "stopped",
        LifecycleState::Error => "error",
    }
}
