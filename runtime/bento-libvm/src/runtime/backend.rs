use super::{local::LocalRuntime, remote::RemoteRuntime};

#[derive(Debug, Clone)]
pub(crate) enum RuntimeBackend {
    Local(Box<LocalRuntime>),
    Remote(RemoteRuntime),
}
