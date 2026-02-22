use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
use clap::Args;
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
        let manager = InstanceManager::new(NixDaemon::new("123"));
        let inst = manager.inspect(&self.name)?;
        manager.stop(&inst)?;
        Ok(())
    }
}
