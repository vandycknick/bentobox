use bento_runtime::instance::InstanceStatus;
use bento_runtime::instance_store::InstanceStore;
use clap::Args;
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};

use crate::daemon_control::signal_instance_stop;

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub async fn run(&self, store: &InstanceStore) -> eyre::Result<()> {
        let inst = store.inspect(&self.name)?;
        signal_instance_stop(&inst)?;

        let timeout = Duration::from_secs(45);
        let poll_interval = Duration::from_millis(200);
        let deadline = Instant::now() + timeout;

        loop {
            let latest = store.inspect(&self.name)?;
            if latest.status() == InstanceStatus::Stopped {
                break;
            }

            if Instant::now() >= deadline {
                return Err(eyre::eyre!(
                    "timed out after {:?} waiting for instance {:?} to stop",
                    timeout,
                    self.name
                ));
            }

            tokio::time::sleep(poll_interval).await;
        }

        Ok(())
    }
}
