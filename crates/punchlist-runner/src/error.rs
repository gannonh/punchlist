//! Runner errors. Shaped like the variants of Symphony's `SymphonyError` that the copied
//! `path_safety.rs` and `workspace.rs` use, so those files change only their import.

#[derive(Debug, thiserror::Error)]
pub enum SymphonyError {
    #[error("workspace {workspace} is not under root {root}")]
    WorkspaceOutsideRoot { workspace: String, root: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SymphonyError>;
