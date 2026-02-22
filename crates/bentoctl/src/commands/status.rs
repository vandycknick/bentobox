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
        // TODO: implement status output by inspecting instance config and daemon liveness.

        Ok(())
    }
}
