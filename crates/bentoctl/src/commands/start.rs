use bento_libvm::{LibVm, MachineRecord, MachineRef};
use clap::Args;
use std::fmt::{Display, Formatter};

use crate::service_readiness;

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,

    #[arg(long = "profile", value_name = "PROFILE")]
    pub profiles: Vec<String>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let machine_ref = MachineRef::parse(self.name.clone())?;
        let machine = libvm.start(&machine_ref, &self.profiles).await?;

        if requires_guest_readiness(&machine, &self.profiles) {
            service_readiness::wait_for_guest_running(
                &machine.dir.join("id.sock"),
                service_readiness::DEFAULT_SERVICE_READINESS_TIMEOUT,
            )
            .await
            .map_err(|err| eyre::eyre!("guest capability readiness check failed: {err}"))?;
        }

        Ok(())
    }
}

fn requires_guest_readiness(machine: &MachineRecord, extra_profiles: &[String]) -> bool {
    let capabilities = &machine.spec.guest.capabilities;

    machine.spec.boot.bootstrap.is_some()
        || machine.spec.host.rosetta
        || !machine.spec.guest.profiles.is_empty()
        || !extra_profiles.is_empty()
        || capabilities.ssh
        || capabilities.dns
        || capabilities.forward
        || capabilities.docker
}
