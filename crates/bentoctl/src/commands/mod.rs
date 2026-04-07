use bento_libvm::LibVm;
use clap::{Parser, Subcommand};
use std::fmt::{Display, Formatter};

use eyre::Context;

pub mod create;
pub mod create_raw;
pub mod delete;
pub mod exec;
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
    Create(create::Cmd),
    #[command(name = "new", hide = true)]
    New(create::Cmd),
    CreateRaw(create_raw::Cmd),
    Start(start::Cmd),
    Stop(stop::Cmd),
    Shell(shell::Cmd),
    Exec(exec::Cmd),
    Delete(delete::Cmd),
    List(list::Cmd),
    Status(status::Cmd),
    #[command(name = "vmmon", alias = "instanced", hide = true)]
    Instanced(instanced::Cmd),
    #[command(name = "images", alias = "image")]
    Images(images::Cmd),
    #[command(hide = true)]
    ShellProxy(shell_proxy::Cmd),
}

impl Display for Command {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Create(cmd) => write!(f, "create {}", cmd),
            Command::New(cmd) => write!(f, "new {}", cmd),
            Command::CreateRaw(cmd) => write!(f, "create-raw {}", cmd),
            Command::Start(cmd) => write!(f, "start {}", cmd),
            Command::Stop(cmd) => write!(f, "stop {}", cmd),
            Command::Shell(cmd) => write!(f, "shell {}", cmd),
            Command::Exec(cmd) => write!(f, "exec {}", cmd),
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
            Command::Create(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::New(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::CreateRaw(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::Start(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::Stop(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::Shell(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::Exec(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::Delete(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::List(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
            Command::Status(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }

            Command::Instanced(cmd) => cmd.run().await,

            Command::Images(cmd) => cmd.run().await,
            Command::ShellProxy(cmd) => {
                let libvm = libvm()?;
                cmd.run(&libvm).await
            }
        }
    }
}

fn libvm() -> eyre::Result<LibVm> {
    LibVm::from_env().context("initialize bento-libvm")
}
