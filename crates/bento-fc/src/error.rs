use thiserror::Error;

#[derive(Debug, Error)]
pub enum FirecrackerError {
    #[error("bento-fc is not implemented yet")]
    Unimplemented,
}
