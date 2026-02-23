use std::fmt::{Display, Formatter};
use std::io::Write;

use bento_runtime::instance::InstanceStatus;
use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
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
    pub fn run(&self) -> eyre::Result<()> {
        let manager = InstanceManager::new(NixDaemon::new("123"));
        let instances = manager.list()?;
        let host_arch = std::env::consts::ARCH;

        let mut out = TabWriter::new(std::io::stdout()).padding(2);
        writeln!(&mut out, "NAME\tSTATUS\tARCH\tCPUS\tMEMORY")?;

        for instance in instances {
            let cpus = instance
                .config
                .cpus
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let memory = instance
                .config
                .memory
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());

            writeln!(
                &mut out,
                "{}\t{}\t{}\t{}\t{}",
                instance.name,
                status_label(instance.status()),
                host_arch,
                cpus,
                memory,
            )?;
        }

        out.flush()?;

        Ok(())
    }
}

fn status_label(status: InstanceStatus) -> &'static str {
    match status {
        InstanceStatus::Running => "running",
        InstanceStatus::Stopped => "stopped",
        InstanceStatus::Broken => "broken",
        InstanceStatus::Unknown => "unknown",
    }
}
