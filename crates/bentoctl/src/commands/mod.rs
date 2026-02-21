use clap::{Parser, Subcommand};
use std::fmt::{Display, Formatter};

pub mod create;
pub mod delete;
pub mod images;
pub mod instanced;
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
    Start(start::Cmd),
    Stop(stop::Cmd),
    Delete(delete::Cmd),
    Status(status::Cmd),
    Instanced(instanced::Cmd),
    #[command(name = "images", alias = "image")]
    Images(images::Cmd),
}

impl Display for Command {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Create(cmd) => write!(f, "create {}", cmd),
            Command::Start(cmd) => write!(f, "start {}", cmd),
            Command::Stop(cmd) => write!(f, "stop {}", cmd),
            Command::Delete(cmd) => write!(f, "delete {}", cmd),
            Command::Status(cmd) => write!(f, "status {}", cmd),
            Command::Instanced(cmd) => write!(f, "instanced {}", cmd),
            Command::Images(cmd) => write!(f, "images {}", cmd),
        }
    }
}

impl BentoCtlCmd {
    pub fn run(&self) -> eyre::Result<()> {
        self.invoke_sub_command()
    }

    fn invoke_sub_command(&self) -> eyre::Result<()> {
        match &self.cmd {
            Command::Create(cmd) => cmd.run(),
            Command::Start(cmd) => cmd.run(),
            Command::Stop(cmd) => cmd.run(),
            Command::Delete(cmd) => cmd.run(),
            Command::Status(cmd) => cmd.run(),

            Command::Instanced(cmd) => cmd.run(),

            Command::Images(cmd) => cmd.run(),
        }
    }
}
