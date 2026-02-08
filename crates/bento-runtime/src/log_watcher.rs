use std::{
    fs::File,
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub struct LogLine {
    // NOTE: note sure I really care about this
    pub stream: StreamKind,
    pub text: String,
}

#[derive(Debug)]
pub enum WatchError {
    TimedOut,
    Io(io::Error),
}

pub struct LogWatcher {
    rx: mpsc::Receiver<Result<LogLine, WatchError>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LogWatcher {
    pub fn spawn(
        stdout_path: PathBuf,
        stderr_path: PathBuf,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Result<LogLine, WatchError>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            let mut stdout_offset = 0_u64;
            let mut stderr_offset = 0_u64;

            loop {
                if stop_thread.load(Ordering::Relaxed) {
                    break;
                }

                if Instant::now() >= deadline {
                    let _ = tx.send(Err(WatchError::TimedOut));
                    break;
                }

                match read_new_lines(&stdout_path, &mut stdout_offset) {
                    Ok(lines) => {
                        for text in lines {
                            let _ = tx.send(Ok(LogLine {
                                stream: StreamKind::Stdout,
                                text,
                            }));
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(WatchError::Io(err)));
                        break;
                    }
                }

                match read_new_lines(&stderr_path, &mut stderr_offset) {
                    Ok(lines) => {
                        for text in lines {
                            let _ = tx.send(Ok(LogLine {
                                stream: StreamKind::Stderr,
                                text,
                            }));
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(WatchError::Io(err)));
                        break;
                    }
                }

                thread::sleep(poll_interval);
            }
        });

        Self {
            rx,
            stop,
            handle: Some(handle),
        }
    }
    pub fn recv(&self) -> Result<Result<LogLine, WatchError>, mpsc::RecvError> {
        self.rx.recv()
    }

    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for LogWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_new_lines(path: &Path, offset: &mut u64) -> io::Result<Vec<String>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(*offset))?;

    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        *offset += n as u64;
        lines.push(line);
    }

    Ok(lines)
}
