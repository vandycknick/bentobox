use bento_runtime::instance_manager::{InstanceCreateOptions, InstanceManager, NixDaemon};
use clap::Args;
use eyre::Context;
use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
};

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,
    #[arg(long, default_value_t = 1, help = "number of virtual CPUs")]
    pub cpus: u8,
    #[arg(long, help = "virtual machine RAM size in mibibytes")]
    pub memory: Option<u32>,
    #[arg(long, help = "Path to a custom kernel, only works for Linux.")]
    pub kernel: Option<PathBuf>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub fn run(&self) -> eyre::Result<()> {
        let daemon = NixDaemon::new("123");
        let manager = InstanceManager::new(daemon);

        let kernel_path = match &self.kernel {
            Some(path) => {
                let abs = if path.is_absolute() {
                    path.clone()
                } else {
                    std::env::current_dir()?.join(path)
                };

                let abs = std::fs::canonicalize(&abs)
                    .context(format!("kernel path does not exist: {}", abs.display()))?;
                Some(abs)
            }
            None => None,
        };

        let options = InstanceCreateOptions::default()
            .with_cpus(self.cpus)
            .with_kernel(kernel_path);

        manager.create(&self.name, options)?;

        println!("created {}", self.name);
        Ok(())
    }
}
