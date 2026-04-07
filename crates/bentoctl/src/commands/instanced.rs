use bento_libvm::{LibVm, MachineRef};
use bento_vmmon::daemon::VmMon;
use clap::Args;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct Cmd {
    #[arg(long = "data-dir")]
    pub data_dir: Option<PathBuf>,

    #[arg(long)]
    pub name: Option<String>,

    #[arg(long = "profile", value_name = "PROFILE")]
    pub profiles: Vec<String>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(data_dir) = &self.data_dir {
            write!(f, "{}", data_dir.display())
        } else if let Some(name) = &self.name {
            write!(f, "{name}")
        } else {
            write!(f, "<missing vmmon target>")
        }
    }
}

impl Cmd {
    pub async fn run(&self) -> eyre::Result<()> {
        let data_dir = match (&self.data_dir, &self.name) {
            (Some(data_dir), None) => data_dir.clone(),
            (None, Some(name)) => {
                let libvm = LibVm::from_env()?;
                let machine = libvm.inspect(&MachineRef::parse(name.clone())?)?;
                machine.dir
            }
            (Some(_), Some(_)) => eyre::bail!("--data-dir and --name are mutually exclusive"),
            (None, None) => eyre::bail!("either --data-dir or --name is required"),
        };

        VmMon::new(data_dir, self.profiles.clone()).run(None).await
    }
}
