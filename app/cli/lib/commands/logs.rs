use std::io::{Read, Seek, SeekFrom, Write};
use std::time::Duration;

use clap::Args;

use crate::context::Context;

#[derive(Debug, Args)]
#[command(about = "Show VM logs")]
pub struct Cmd {
    /// Name or ID of the VM whose logs should be shown. Defaults to the configured default VM.
    #[arg(value_name = "VM")]
    name: Option<String>,

    /// Continue streaming logs as they are written.
    #[arg(long)]
    follow: bool,
}

impl Cmd {
    pub async fn run(self, context: &mut Context) -> eyre::Result<()> {
        let (_name, machine) = context.machine(self.name.as_deref()).await?;
        let data = machine.inspect().await?;
        let path = data.trace_log_path();
        if !path.exists() {
            return Ok(());
        }

        if !self.follow {
            let bytes = std::fs::read(path)?;
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            out.write_all(&bytes)?;
            out.flush()?;
            return Ok(());
        }

        let mut file = std::fs::File::open(&path)?;
        let mut position = file.seek(SeekFrom::Start(0))?;

        loop {
            file.seek(SeekFrom::Start(position))?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)?;
            if !buffer.is_empty() {
                position = position.saturating_add(u64::try_from(buffer.len()).unwrap_or(u64::MAX));
                let stdout = std::io::stdout();
                let mut out = stdout.lock();
                out.write_all(&buffer)?;
                out.flush()?;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
