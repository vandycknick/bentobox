use bento_runtime::image_store::ImageStore;
use bento_runtime::instance::InstanceFile;
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
    #[arg(
        long,
        default_value_t = 512,
        help = "virtual machine RAM size in mibibytes"
    )]
    pub memory: u32,
    #[arg(long, help = "Path to a custom kernel, only works for Linux.")]
    pub kernel: Option<PathBuf>,
    #[arg(long, help = "Base image name or OCI reference")]
    pub image: Option<String>,
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
        let mut store = ImageStore::open()?;

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
            .with_memory(self.memory)
            .with_kernel(kernel_path);

        let selected_image = self
            .image
            .as_deref()
            .map(|image_arg| -> eyre::Result<_> {
                Ok(match store.resolve(image_arg)? {
                    Some(image) => image,
                    None => store.pull(image_arg, None)?,
                })
            })
            .transpose()?;

        let inst = manager.create(&self.name, options)?;

        if let Some(image) = selected_image {
            store.clone_base_image(&image, &inst.file(InstanceFile::RootDisk))?;
        }

        println!("created {}", self.name);
        Ok(())
    }
}
