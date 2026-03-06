use bento_runtime::instance_store::InstanceStore;
use clap::Args;
use std::fmt::{Display, Formatter};

use crate::daemon_control::{launch_instance, InstancedLauncher};

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
    pub async fn run(
        &self,
        store: &InstanceStore,
        mut launcher: InstancedLauncher,
    ) -> eyre::Result<()> {
        let inst = store.inspect(&self.name)?;
        launch_instance(&mut launcher, &inst).await?;
        Ok(())
    }
}
