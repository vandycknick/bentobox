use std::fmt::{Display, Formatter};

use bento_runtime::instance_store::InstanceStore;
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
    pub async fn run(&self, store: &InstanceStore) -> eyre::Result<()> {
        let inst = store.inspect(&self.name)?;

        store.delete(&inst)?;

        println!("deleted {}", self.name);
        Ok(())
    }
}
