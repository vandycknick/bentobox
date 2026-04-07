use std::fs::File;
use std::io::{self, Write};
use std::os::fd::FromRawFd;

pub struct StartupReporter {
    file: Option<File>,
}

impl StartupReporter {
    pub fn from_raw_fd(fd: i32) -> Self {
        let file = unsafe { File::from_raw_fd(fd) };
        Self { file: Some(file) }
    }

    pub fn report_started(&mut self) -> io::Result<()> {
        self.write_message("started\n")
    }

    pub fn report_failed(&mut self, message: &str) -> io::Result<()> {
        self.write_message(&format!("failed\t{message}\n"))
    }

    fn write_message(&mut self, message: &str) -> io::Result<()> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };
        file.write_all(message.as_bytes())?;
        file.flush()?;
        Ok(())
    }
}
