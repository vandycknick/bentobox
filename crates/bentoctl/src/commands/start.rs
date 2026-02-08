use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
use clap::Args;
use eyre::Context;
use std::fmt::{Display, Formatter};

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
    pub fn run(&self) -> eyre::Result<()> {
        let exe = std::env::current_exe().context("resolve bentoctl binary path")?;
        let daemon = NixDaemon::new(exe)
            .arg("instanced")
            .arg("--name")
            .arg(&self.name);

        let mut manager = InstanceManager::new(daemon);

        let inst = manager.inspect(&self.name)?;
        manager.start(&inst)?;
        Ok(())
    }
}
