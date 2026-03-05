use std::fmt::{Display, Formatter};

use bento_instanced::launcher::NixLauncher;
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
    pub async fn run(&self, manager: &InstanceManager<NixLauncher>) -> eyre::Result<()> {
        let inst = manager.inspect(&self.name)?;

        manager.delete(&inst)?;

        println!("deleted {}", self.name);
        Ok(())
    }
}
