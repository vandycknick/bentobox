use bento_instanced::daemon::InstanceDaemon;
use clap::Args;
use std::fmt::{Display, Formatter};

#[derive(Args, Debug)]
pub struct Cmd {
    #[arg(long)]
    pub name: String,

    #[arg(long = "profile", value_name = "PROFILE")]
    pub profiles: Vec<String>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub async fn run(&self) -> eyre::Result<()> {
        InstanceDaemon::new(&self.name, self.profiles.clone())
            .run()
            .await
    }
}
