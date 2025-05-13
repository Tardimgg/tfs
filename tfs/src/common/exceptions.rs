use thiserror::Error;
use crate::common::exceptions::ApiException::InternalError;
use crate::services::file_storage::model::FolderMeta;
use crate::services::virtual_fs::models::GlobalFileInfo;

#[derive(Debug)]
pub enum ApiException {
    FileNotFound,
    NotFound,
    FolderNotFound,
    FileAlreadyExist,
    FolderAlreadyExist,
    BadRequest(String),
    Forbidden(String),
    InternalError(String),
    UnknownNode(String)
}

impl From<String> for ApiException {
    fn from(value: String) -> Self {
        InternalError(value)
    }
}

#[derive(Error, Debug)]
pub enum NodeMetaReceivingError {
    #[error("node not found")]
    NotFound,
    #[error("internal error")]
    InternalError(String),
    #[error("access denied")]
    AccessDenied
}

impl From<String> for NodeMetaReceivingError {
    fn from(value: String) -> Self {
        NodeMetaReceivingError::InternalError(value)
    }
}
