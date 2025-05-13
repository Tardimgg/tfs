use thiserror::Error;

#[derive(Error, Debug)]
pub enum UserPermissionsReadingError {
    #[error("access denied")]
    AccessDenied,
    #[error("internal error: {0}")]
    InternalError(String)
}


impl From<String> for UserPermissionsReadingError {
    fn from(value: String) -> Self {
        UserPermissionsReadingError::InternalError(value)
    }
}