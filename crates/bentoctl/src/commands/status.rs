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
        // ensure_valid_name(&self.name)?;
        //
        // let paths =
        //     bento_util::instance::instance_paths(&self.name).context("resolve instance paths")?;
        // if !paths.root_dir.exists() {
        //     return Err(anyhow!("instance {} does not exist", self.name));
        // }
        //
        // if !paths.config_path.exists() {
        //     return Err(anyhow!("instance {} is missing config.yaml", self.name));
        // }
        //
        // let running = read_instanced_pid(&paths)?.is_some();
        // if running {
        //     println!("status: running");
        // } else {
        //     println!("status: stopped");
        // }

        Ok(())
    }
}
