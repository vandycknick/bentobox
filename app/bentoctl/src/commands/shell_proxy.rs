use std::fmt::{Display, Formatter};

use bento_libvm::{LibVm, MachineRef};
use clap::Args;
use eyre::Context;
use tokio::io::AsyncWriteExt;

#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Cmd {
    #[arg(long)]
    pub name: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "--name {}", self.name)
    }
}

impl Cmd {
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let stream = libvm
            .open_shell_stream(&MachineRef::parse(self.name.clone())?, true)
            .await
            .context("open negotiated shell stream")?;
        proxy_stdio(stream).await
    }
}

async fn proxy_stdio(stream: tokio::net::UnixStream) -> eyre::Result<()> {
    let (mut stream_read, mut stream_write) = stream.into_split();

    let input = async {
        let mut stdin = tokio::io::stdin();
        tokio::io::copy(&mut stdin, &mut stream_write)
            .await
            .context("relay shell input")?;
        stream_write
            .shutdown()
            .await
            .context("shutdown shell input stream")?;
        Ok::<(), eyre::Report>(())
    };

    let output = async {
        let mut stdout = tokio::io::stdout();
        tokio::io::copy(&mut stream_read, &mut stdout)
            .await
            .context("relay shell output")?;
        stdout.flush().await.context("flush shell output")?;
        Ok::<(), eyre::Report>(())
    };

    tokio::try_join!(output, input)?;
    Ok(())
}
