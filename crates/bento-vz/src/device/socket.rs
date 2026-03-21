use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use block2::StackBlock;
use nix::unistd::dup;
use objc2::{rc::Retained, ClassType};
use objc2_virtualization::{
    VZSocketDevice, VZSocketDeviceConfiguration, VZVirtioSocketConnection, VZVirtioSocketDevice,
    VZVirtioSocketDeviceConfiguration, VZVirtualMachine,
};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::oneshot;

use crate::dispatch::Queue;
use crate::error::VzError;

#[allow(async_fn_in_trait)]
pub trait SocketDevice: Send + Sync {
    type Connection: AsyncRead + AsyncWrite + Send + Unpin + 'static;
    type Listener;

    async fn connect(&self, port: u32) -> Result<Self::Connection, VzError>;
    fn listen(&self, port: u32) -> Result<Self::Listener, VzError>;
}

#[derive(Debug, Clone)]
pub struct SocketDeviceConfiguration {
    inner: Retained<VZVirtioSocketDeviceConfiguration>,
}

impl SocketDeviceConfiguration {
    pub fn new() -> Self {
        Self {
            inner: unsafe { VZVirtioSocketDeviceConfiguration::new() },
        }
    }

    pub(crate) fn as_inner(&self) -> &VZSocketDeviceConfiguration {
        self.inner.as_super()
    }
}

impl Default for SocketDeviceConfiguration {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct VirtioSocketDevice {
    machine: Retained<VZVirtualMachine>,
    queue: Queue,
    index: usize,
}

// SAFETY: The device is only touched via the VM's serial dispatch queue.
unsafe impl Send for VirtioSocketDevice {}
// SAFETY: See above.
unsafe impl Sync for VirtioSocketDevice {}

impl VirtioSocketDevice {
    pub(crate) fn new(machine: Retained<VZVirtualMachine>, queue: Queue, index: usize) -> Self {
        Self {
            machine,
            queue,
            index,
        }
    }
}

impl SocketDevice for VirtioSocketDevice {
    type Connection = VirtioSocketConnection;
    type Listener = VirtioSocketListener;

    async fn connect(&self, port: u32) -> Result<Self::Connection, VzError> {
        let machine = self.machine.clone();
        let queue = self.queue.clone();
        let index = self.index;
        let (sender, receiver) = oneshot::channel();
        let shared_sender = Arc::new(Mutex::new(Some(sender)));

        queue.exec_block_async(&StackBlock::new(move || unsafe {
            let completion_sender = shared_sender.clone();
            let devices = machine.socketDevices();
            if index >= devices.count() {
                send_completion_once(
                    &completion_sender,
                    Err(VzError::Backend(
                        "socket device is no longer available".to_string(),
                    )),
                );
                return;
            }
            let device: Retained<VZSocketDevice> = devices.objectAtIndex(index);
            let Some(vsock) = device.downcast_ref::<VZVirtioSocketDevice>() else {
                send_completion_once(
                    &completion_sender,
                    Err(VzError::Backend(
                        "socket device is not a virtio socket device".to_string(),
                    )),
                );
                return;
            };

            let completion_handler = StackBlock::new(
                move |connection: *mut VZVirtioSocketConnection,
                      err: *mut objc2_foundation::NSError| {
                    let err = err.as_ref();
                    if let Some(error) = err {
                        send_completion_once(
                            &completion_sender,
                            Err(VzError::Backend(error.localizedDescription().to_string())),
                        );
                        return;
                    }

                    let Some(connection) = connection.as_ref() else {
                        send_completion_once(
                            &completion_sender,
                            Err(VzError::Backend(
                                "vsock connection completed without a connection object"
                                    .to_string(),
                            )),
                        );
                        return;
                    };

                    let file_descriptor = connection.fileDescriptor();
                    let borrowed = BorrowedFd::borrow_raw(file_descriptor);
                    let result = dup(borrowed)
                        .map_err(|err| {
                            VzError::Backend(format!("duplicate vsock file descriptor: {err}"))
                        })
                        .and_then(|fd| VirtioSocketConnection::new(fd, port));
                    send_completion_once(&completion_sender, result);
                },
            );

            vsock.connectToPort_completionHandler(port, &completion_handler);
        }));

        receiver.await.map_err(|_| {
            VzError::Backend(
                "vsock completion channel closed before result was delivered".to_string(),
            )
        })?
    }

    fn listen(&self, _port: u32) -> Result<Self::Listener, VzError> {
        Err(VzError::Unimplemented("virtio socket listeners"))
    }
}

pub struct VirtioSocketConnection {
    inner: AsyncFd<std::fs::File>,
    destination_port: u32,
}

impl fmt::Debug for VirtioSocketConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtioSocketConnection")
            .field("fd", &self.inner.get_ref().as_raw_fd())
            .field("destination_port", &self.destination_port)
            .finish()
    }
}

impl VirtioSocketConnection {
    fn new(fd: OwnedFd, destination_port: u32) -> Result<Self, VzError> {
        let file = std::fs::File::from(fd);
        super::serial::set_nonblocking(&file)?;
        Ok(Self {
            inner: AsyncFd::new(file)?,
            destination_port,
        })
    }

    pub fn destination_port(&self) -> u32 {
        self.destination_port
    }
}

impl AsyncRead for VirtioSocketConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let bytes =
            unsafe { &mut *(buf.unfilled_mut() as *mut [std::mem::MaybeUninit<u8>] as *mut [u8]) };
        loop {
            let mut guard = ready!(self.inner.poll_read_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().read(bytes)) {
                Ok(Ok(n)) => {
                    unsafe { buf.assume_init(n) };
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }
}

impl AsyncWrite for VirtioSocketConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.inner.poll_write_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(Ok(n)) => return Poll::Ready(Ok(n)),
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.get_ref().flush()?;
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
pub struct VirtioSocketListener;

fn send_completion_once<T>(sender: &Arc<Mutex<Option<oneshot::Sender<T>>>>, value: T) {
    if let Some(sender) = sender.lock().ok().and_then(|mut guard| guard.take()) {
        let _ = sender.send(value);
    }
}
