use clap::Args;

use crate::context::Context;
use crate::ui::Spinner;

#[derive(Debug, Args)]
#[command(about = "Stop a persistent VM")]
pub struct Cmd {
    /// Name or ID of the VM to stop. Defaults to the configured default VM.
    #[arg(value_name = "VM")]
    name: Option<String>,

    /// Force stop instead of asking the VM to shut down.
    #[arg(long)]
    force: bool,
}

impl Cmd {
    pub async fn run(self, context: &mut Context) -> eyre::Result<()> {
        let mut spinner = Spinner::start("Finding", self.name.as_deref().unwrap_or("default VM"));
        let (name, machine) = context.machine(self.name.as_deref()).await?;

        if self.force {
            spinner.step("Killing", &name);
            machine.kill().await?;
        } else {
            spinner.step("Stopping", &name);
            machine.stop().await?;
        }

        spinner.step("Stopped", &name);
        spinner.finish_success("Stopped");
        Ok(())
    }
}
