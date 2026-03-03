use bento_instanced::daemon::InstanceDaemon;
use clap::Args;
use eyre::Context;
use std::fmt::{Display, Formatter};

#[derive(Args, Debug)]
pub struct Cmd {
    #[arg(long)]
    pub name: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub fn run(&self) -> eyre::Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("build tokio runtime for instanced")?;

        runtime.block_on(InstanceDaemon::new(&self.name).run())
    }
}
