use bento_runtime::instance_daemon::InstanceDaemon;
use clap::Args;
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
        InstanceDaemon::new(&self.name).run()
    }
}
