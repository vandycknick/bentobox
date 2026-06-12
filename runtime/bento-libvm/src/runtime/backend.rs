use super::local::LocalRuntime;

#[derive(Debug, Clone)]
pub(crate) enum RuntimeBackend {
    Local(LocalRuntime),
}
