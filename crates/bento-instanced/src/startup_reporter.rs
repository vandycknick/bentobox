use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{BorrowedFd, FromRawFd};

pub struct StartupReporter {
    file: Option<File>,
}

impl StartupReporter {
    pub fn from(startup_fd: Option<i32>) -> io::Result<Self> {
        match startup_fd {
            Some(fd) => Ok(Self::from_startup_fd(fd)),
            None => Self::from_stdout(),
        }
    }

    fn from_startup_fd(fd: i32) -> Self {
        let file = unsafe { File::from_raw_fd(fd) };
        Self { file: Some(file) }
    }

    fn from_stdout() -> io::Result<Self> {
        let borrowed = unsafe { BorrowedFd::borrow_raw(libc::STDOUT_FILENO) };
        let duplicated = nix::unistd::dup(borrowed).map_err(io::Error::other)?;
        let file = File::from(duplicated);
        Ok(Self { file: Some(file) })
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
