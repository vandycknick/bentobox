use std::fmt::{Display, Formatter};

use bento_libvm::{LibVm, MachineRef};
use bento_runtime::profiles::ENDPOINT_SSH;
use clap::Args;
use eyre::Context;
use tokio::io::AsyncWriteExt;

#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Cmd {
    #[arg(long)]
    pub name: String,

    #[arg(long, default_value = ENDPOINT_SSH)]
    pub service: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "--name {} --service {}", self.name, self.service)
    }
}

impl Cmd {
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let stream = libvm
            .open_service_stream(
                &MachineRef::parse(self.name.clone())?,
                &self.service,
                self.service == ENDPOINT_SSH,
            )
            .await
            .context("open negotiated monitor proxy stream")?;
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
