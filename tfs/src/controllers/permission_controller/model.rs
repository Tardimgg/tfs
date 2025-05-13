use crate::common::exceptions::ApiException;
use crate::services::permission_service::errors::ReadingUserPermissionsToObjectError;

impl From<ReadingUserPermissionsToObjectError> for ApiException {
    fn from(value: ReadingUserPermissionsToObjectError) -> Self {
        match value {
            ReadingUserPermissionsToObjectError::AccessDenied => ApiException::Forbidden("".to_string()),
            ReadingUserPermissionsToObjectError::InternalError(v) => ApiException::InternalError(v)
        }
    }
}