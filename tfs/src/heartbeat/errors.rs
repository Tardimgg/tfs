use thiserror::Error;

#[derive(Error, Debug)]
pub enum UpdateDhtError {
    #[error("file not found: {0}")]
    FileNotFound(String),
    #[error("internal error: {0}")]
    InternalError(String)
}


impl From<String> for UpdateDhtError {
    fn from(value: String) -> Self {
        UpdateDhtError::InternalError(value)
    }
}