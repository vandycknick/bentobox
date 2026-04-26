use bento_libvm::{LibVm, MachineRef};
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
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let machine = MachineRef::parse(self.name.clone())?;
        libvm.stop(&machine).await?;
        Ok(())
    }
}
