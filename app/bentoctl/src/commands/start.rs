use bento_libvm::{LibVm, MachineRecord, MachineRef};
use clap::Args;
use std::fmt::{Display, Formatter};
use std::time::Duration;

#[derive(Args, Debug)]
#[command(about = "Start a persistent VM")]
pub struct Cmd {
    /// Name or ID of the VM to start.
    #[arg(value_name = "VM")]
    pub name: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let machine_ref = MachineRef::parse(self.name.clone())?;
        let machine = libvm.start(&machine_ref).await?;

        if requires_guest_readiness(&machine) {
            libvm
                .wait_for_guest_running(
                    &MachineRef::Id(machine.id),
                    Duration::from_secs(machine.spec.settings.agent.timeout_seconds),
                )
                .await
                .map_err(|err| eyre::eyre!("guest readiness check failed: {err}"))?;
        }

        Ok(())
    }
}

fn requires_guest_readiness(machine: &MachineRecord) -> bool {
    machine.spec.settings.agent.enabled
}
