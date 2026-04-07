use std::fmt::{Display, Formatter};
use std::io::Write;

use bento_libvm::{LibVm, MachineStatus};
use clap::Args;
use tabwriter::TabWriter;

#[derive(Args, Debug, Default)]
pub struct Cmd {}

impl Display for Cmd {
    fn fmt(&self, _f: &mut Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

impl Cmd {
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let machines = libvm.list()?;
        let host_arch = std::env::consts::ARCH;

        let mut out = TabWriter::new(std::io::stdout()).padding(2);
        writeln!(&mut out, "NAME\tSTATUS\tARCH\tCPUS\tMEMORY")?;

        for machine in machines {
            let cpus = Some(machine.spec.resources.cpus)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let memory = Some(machine.spec.resources.memory_mib)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());

            writeln!(
                &mut out,
                "{}\t{}\t{}\t{}\t{}",
                machine.spec.name,
                status_label(machine.status),
                host_arch,
                cpus,
                memory,
            )?;
        }

        out.flush()?;

        Ok(())
    }
}

fn status_label(status: MachineStatus) -> &'static str {
    match status {
        MachineStatus::Running => "running",
        MachineStatus::Stopped => "stopped",
    }
}
