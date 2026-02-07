use std::process::ExitCode;

use axum::Router;
use bentobox::api;
use clap::Parser;
use eyre::Context;
use tokio::net::{TcpListener, UnixListener};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Bentod {
    #[arg(long, short, default_value = "/tmp/bentod.sock")]
    socket: Option<String>,

    #[arg(
        long,
        short = 'v',
        action = clap::ArgAction::Count,
        global = true,
        help = "Write verbose messages to stderr for debugging.",
        display_order = 999
    )]
    verbose: u8,
}

#[derive(Debug, Clone)]
pub struct Test {}

impl Bentod {
    async fn create_server(&self, socket: &str) -> eyre::Result<()> {
        let app = Router::new().nest("/api", api::create_router());

        let listener = UnixListener::bind(socket)
            .with_context(|| format!("Failed binding to socket {}.", socket))?;

        axum::serve(listener, app)
            .await
            .context("Failed to start axum server.")
    }

    pub fn run(&self) -> eyre::Result<()> {
        eprintln!("{:?}", self.socket);
        eprintln!("{:?}", self.verbose);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let socket = match &self.socket {
            Some(socket) => socket.clone(),
            None => "/tmp/bentod.sock".to_string(),
        };

        runtime.block_on(self.create_server(&socket))
    }
}

fn main() -> ExitCode {
    let daemon = Bentod::parse();

    match daemon.run() {
        Err(err) => {
            let root = err.root_cause();

            eprint!("\x1b[31m");
            eprintln!("Error: {}", err);
            eprintln!("");
            eprintln!("Caused by:");
            eprint!("  {}", root);
            eprintln!("\x1b[0m");
            ExitCode::from(1)
        }
        Ok(_) => ExitCode::from(0),
    }
}
