use crate::engine::LocalRuntime;

#[derive(Debug, Clone)]
pub(crate) enum RuntimeBackend {
    Local(LocalRuntime),
}
