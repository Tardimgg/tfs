use thiserror::Error;
use crate::services::rebac::errors::UserPermissionsReadingError;

#[derive(Error, Debug)]
pub enum ReadingUserPermissionsToObjectError {
    #[error("access denied")]
    AccessDenied,
    #[error("internal error: {0}")]
    InternalError(String)
}


impl From<String> for ReadingUserPermissionsToObjectError {
    fn from(value: String) -> Self {
        ReadingUserPermissionsToObjectError::InternalError(value)
    }
}

impl From<UserPermissionsReadingError> for ReadingUserPermissionsToObjectError {
    fn from(value: UserPermissionsReadingError) -> Self {
        match value {
            UserPermissionsReadingError::AccessDenied => ReadingUserPermissionsToObjectError::AccessDenied,
            UserPermissionsReadingError::InternalError(err) => ReadingUserPermissionsToObjectError::InternalError(err)
        }
    }
}