use std::path::PathBuf;

use crate::{
    config::InstanceConfig,
    driver::{Driver, DriverError},
};

#[derive(Debug, Clone)]
pub struct FirecrackerDriver {
    _instance_dir: PathBuf,
}

impl FirecrackerDriver {
    pub fn new(instance_dir: PathBuf) -> Self {
        Self {
            _instance_dir: instance_dir,
        }
    }
}

impl Driver for FirecrackerDriver {
    fn name(&self) -> &'static str {
        "firecracker"
    }

    fn create(&mut self, _config: &InstanceConfig) -> Result<(), DriverError> {
        Ok(())
    }

    fn start(&mut self, _config: &InstanceConfig) -> Result<(), DriverError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}
