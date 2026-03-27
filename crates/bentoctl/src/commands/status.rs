use clap::Args;
use std::fmt::{Display, Formatter};

use bento_protocol::v1::LifecycleState;
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

        if !status.capabilities.is_empty() {
            println!("capabilities:");
            for capability in status.capabilities {
                println!(
                    "  - {} enabled={} startup_required={} configured={} running={}",
                    capability.name,
                    capability.enabled,
                    capability.startup_required,
                    capability.configured,
                    capability.running,
                );
                if !capability.summary.is_empty() {
                    println!("    summary: {}", capability.summary);
                }
                for problem in capability.problems {
                    println!("    problem: {}", problem);
                }
            }
        }

        if !status.endpoints.is_empty() {
            println!("endpoints:");
            for endpoint in status.endpoints {
                println!(
                    "  - {} guest={} host={} active={}",
                    endpoint.name, endpoint.guest_address, endpoint.host_address, endpoint.active
                );
                if !endpoint.summary.is_empty() {
                    println!("    summary: {}", endpoint.summary);
                }
                for problem in endpoint.problems {
                    println!("    problem: {}", problem);
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
