use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::sync::mpsc;

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
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl LogWatcher {
    pub fn spawn(
        stdout_path: PathBuf,
        stderr_path: PathBuf,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Result<LogLine, WatchError>>(256);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_task = Arc::clone(&stop);

        let handle = tokio::spawn(async move {
            let deadline = Instant::now() + timeout;
            let mut stdout_offset = 0_u64;
            let mut stderr_offset = 0_u64;

            loop {
                if stop_task.load(Ordering::Relaxed) {
                    break;
                }

                if Instant::now() >= deadline {
                    let _ = tx.send(Err(WatchError::TimedOut)).await;
                    break;
                }

                match read_new_lines(&stdout_path, &mut stdout_offset).await {
                    Ok(lines) => {
                        for text in lines {
                            if tx
                                .send(Ok(LogLine {
                                    stream: StreamKind::Stdout,
                                    text,
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(WatchError::Io(err))).await;
                        break;
                    }
                }

                match read_new_lines(&stderr_path, &mut stderr_offset).await {
                    Ok(lines) => {
                        for text in lines {
                            if tx
                                .send(Ok(LogLine {
                                    stream: StreamKind::Stderr,
                                    text,
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(WatchError::Io(err))).await;
                        break;
                    }
                }

                tokio::time::sleep(poll_interval).await;
            }
        });

        Self {
            rx,
            stop,
            handle: Some(handle),
        }
    }

    pub async fn recv(&mut self) -> Option<Result<LogLine, WatchError>> {
        self.rx.recv().await
    }

    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for LogWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

async fn read_new_lines(path: &Path, offset: &mut u64) -> io::Result<Vec<String>> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut reader = BufReader::new(file);
    reader
        .seek(std::io::SeekFrom::Start(*offset))
        .await
        .map_err(io::Error::other)?;

    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        *offset += n as u64;
        lines.push(line);
    }

    Ok(lines)
}
