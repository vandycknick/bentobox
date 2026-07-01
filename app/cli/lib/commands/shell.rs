use clap::{Args, ValueEnum};
use eyre::bail;
use libvm::MachineData;

use crate::context::Context;
use crate::ssh;
use crate::terminal;
use crate::ui;

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
pub enum AttachMode {
    Shell,
    Serial,
}

#[derive(Debug, Args)]
#[command(about = "Open a shell in a running VM")]
pub struct Cmd {
    /// Name or ID of the running VM. Defaults to the configured default VM.
    #[arg(value_name = "VM")]
    pub name: Option<String>,

    /// Guest user for the shell session.
    #[arg(long, short = 'u')]
    pub user: Option<String>,

    /// Attach through the guest shell or serial console.
    #[arg(long, value_enum)]
    pub attach: Option<AttachMode>,
}

impl Cmd {
    pub async fn run(self, context: &mut Context) -> eyre::Result<()> {
        let (_reference, machine) = context.machine(self.name.as_deref()).await?;
        let inspect_data = machine.inspect().await?;
        let machine_name = inspect_data.name.clone();

        ensure_running(&inspect_data)?;

        if self.attach == Some(AttachMode::Serial) {
            if self.user.is_some() {
                ui::warn("--user is ignored for serial attach");
            }
            let stream = machine.open_serial_stream().await?;
            return terminal::attach_serial_stream(stream).await;
        }

        ensure_guest_ready(&inspect_data)?;
        let data_dir = context.runtime().await?.local_data_dir().to_path_buf();
        ssh::exec_remote_shell(&data_dir, &machine_name, self.user.as_deref())
    }
}

fn ensure_running(data: &MachineData) -> eyre::Result<()> {
    if data.is_running() {
        return Ok(());
    }

    Err(eyre::eyre!(
        "machine `{}` is not running; start it with `bento start {}`",
        data.name,
        data.name
    ))
}

fn ensure_guest_ready(data: &MachineData) -> eyre::Result<()> {
    if data.status.guest_ready() {
        return Ok(());
    }

    let summary = data
        .status
        .message()
        .map(str::to_string)
        .unwrap_or_else(|| format!("machine state is {}", data.status.label()));
    bail!("guest service is not ready: {summary}");
}
