use std::fmt::{Display, Formatter};

use bento_instanced::daemon::NixDaemon;
use bento_runtime::instance_manager::InstanceManager;
use clap::Args;

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
    pub async fn run(&self) -> eyre::Result<()> {
        let daemon = NixDaemon::new("123");
        let manager = InstanceManager::new(daemon);
        let inst = manager.inspect(&self.name)?;

        manager.delete(&inst)?;

        println!("deleted {}", self.name);
        Ok(())
    }
}
