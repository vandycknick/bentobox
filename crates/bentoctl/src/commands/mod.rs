use clap::{Parser, Subcommand};
use std::fmt::{Display, Formatter};

use bento_runtime::instance_store::InstanceStore;
use eyre::Context;

use crate::daemon_control::InstancedLauncher;

pub mod create;
pub mod delete;
pub mod images;
pub mod instanced;
pub mod list;
pub mod shell;
pub mod shell_proxy;
pub mod start;
pub mod status;
pub mod stop;

#[derive(Parser)]
#[command(
    name = "bentoctl",
    about = "Bentobox instance lifecycle control",
    disable_help_subcommand = true
)]
pub struct BentoCtlCmd {
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    New(create::Cmd),
    #[command(name = "create", hide = true)]
    CreateAlias(create::Cmd),
    Start(start::Cmd),
    Stop(stop::Cmd),
    Shell(shell::Cmd),
    Delete(delete::Cmd),
    List(list::Cmd),
    Status(status::Cmd),
    Instanced(instanced::Cmd),
    #[command(name = "images", alias = "image")]
    Images(images::Cmd),
    #[command(hide = true)]
    ShellProxy(shell_proxy::Cmd),
}

impl Display for Command {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::New(cmd) => write!(f, "new {}", cmd),
            Command::CreateAlias(cmd) => write!(f, "new {}", cmd),
            Command::Start(cmd) => write!(f, "start {}", cmd),
            Command::Stop(cmd) => write!(f, "stop {}", cmd),
            Command::Shell(cmd) => write!(f, "shell {}", cmd),
            Command::Delete(cmd) => write!(f, "delete {}", cmd),
            Command::List(_) => write!(f, "list"),
            Command::Status(cmd) => write!(f, "status {}", cmd),
            Command::Instanced(cmd) => write!(f, "instanced {}", cmd),
            Command::Images(cmd) => write!(f, "images {}", cmd),
            Command::ShellProxy(cmd) => write!(f, "shell-proxy {}", cmd),
        }
    }
}

impl BentoCtlCmd {
    pub async fn run(&self) -> eyre::Result<()> {
        self.invoke_sub_command().await
    }

    async fn invoke_sub_command(&self) -> eyre::Result<()> {
        match &self.cmd {
            Command::New(cmd) => {
                let store = instance_store();
                cmd.run(&store).await
            }
            Command::CreateAlias(cmd) => {
                let store = instance_store();
                cmd.run(&store).await
            }
            Command::Start(cmd) => {
                let store = instance_store();
                let launcher = start_launcher(&cmd.name)?;
                cmd.run(&store, launcher).await
            }
            Command::Stop(cmd) => {
                let store = instance_store();
                cmd.run(&store).await
            }
            Command::Shell(cmd) => {
                let store = instance_store();
                cmd.run(&store).await
            }
            Command::Delete(cmd) => {
                let store = instance_store();
                cmd.run(&store).await
            }
            Command::List(cmd) => {
                let store = instance_store();
                cmd.run(&store).await
            }
            Command::Status(cmd) => cmd.run().await,

            Command::Instanced(cmd) => cmd.run().await,

            Command::Images(cmd) => cmd.run().await,
            Command::ShellProxy(cmd) => {
                let store = instance_store();
                cmd.run(&store).await
            }
        }
    }
}

fn instance_store() -> InstanceStore {
    InstanceStore::new()
}

fn start_launcher(name: &str) -> eyre::Result<InstancedLauncher> {
    let exe = std::env::current_exe().context("resolve bentoctl binary path")?;
    Ok(InstancedLauncher::for_instance(exe, name))
}
