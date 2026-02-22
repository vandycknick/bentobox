use crate::{
    driver::{Driver, DriverError},
    instance::Instance,
};

#[derive(Debug, Clone)]
pub struct FirecrackerDriver {
    _instance: Instance,
}

impl FirecrackerDriver {
    pub fn new(instance: Instance) -> Self {
        Self {
            _instance: instance,
        }
    }
}

impl Driver for FirecrackerDriver {
    fn validate(&self) -> Result<(), DriverError> {
        Ok(())
    }

    fn create(&self) -> Result<(), DriverError> {
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}
