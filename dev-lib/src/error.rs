/// Typed errors for user-facing failure modes, distinct from unexpected
/// infrastructure failures (`anyhow::Error`).
#[derive(Debug, thiserror::Error)]
pub enum DevError {
    #[error("project not found: {0}")]
    ProjectNotFound(String),
}
