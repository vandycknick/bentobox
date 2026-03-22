use std::fmt;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::sync::{Arc, Mutex};

use block2::StackBlock;
use nix::unistd::dup;
use objc2::{rc::Retained, ClassType};
use objc2_virtualization::{
    VZSocketDevice, VZSocketDeviceConfiguration, VZVirtioSocketConnection, VZVirtioSocketDevice,
    VZVirtioSocketDeviceConfiguration, VZVirtualMachine,
};
use tokio::sync::oneshot;

use crate::dispatch::Queue;
use crate::error::VzError;

#[allow(async_fn_in_trait)]
pub trait SocketDevice: Send + Sync {
    type Connection: Read + Write + AsRawFd + Send + 'static;
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
                    let source_port = connection.sourcePort();
                    let result = dup(borrowed)
                        .map_err(|err| {
                            VzError::Backend(format!("duplicate vsock file descriptor: {err}"))
                        })
                        .and_then(|fd| VirtioSocketConnection::new(fd, source_port, port));
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
    file: std::fs::File,
    source_port: u32,
    destination_port: u32,
}

impl fmt::Debug for VirtioSocketConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtioSocketConnection")
            .field("fd", &self.file.as_raw_fd())
            .field("source_port", &self.source_port)
            .field("destination_port", &self.destination_port)
            .finish()
    }
}

impl VirtioSocketConnection {
    fn new(fd: OwnedFd, source_port: u32, destination_port: u32) -> Result<Self, VzError> {
        let file = std::fs::File::from(fd);
        super::serial::set_nonblocking(&file)?;
        Ok(Self {
            file,
            source_port,
            destination_port,
        })
    }

    pub fn source_port(&self) -> u32 {
        self.source_port
    }

    pub fn destination_port(&self) -> u32 {
        self.destination_port
    }
}

impl AsRawFd for VirtioSocketConnection {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl Read for VirtioSocketConnection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Read for &VirtioSocketConnection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&self.file).read(buf)
    }
}

impl Write for VirtioSocketConnection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Write for &VirtioSocketConnection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        (&self.file).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (&self.file).flush()
    }
}

#[derive(Debug)]
pub struct VirtioSocketListener;

fn send_completion_once<T>(sender: &Arc<Mutex<Option<oneshot::Sender<T>>>>, value: T) {
    if let Some(sender) = sender.lock().ok().and_then(|mut guard| guard.take()) {
        let _ = sender.send(value);
    }
}
