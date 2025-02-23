use std::fmt::{Debug, Display, Formatter};
use actix_web::ResponseError;
use thiserror::Error;
use crate::controllers::virtual_fs_controller::error::ApiException::{BadRequest, FileAlreadyExist, FileNotFound, FolderAlreadyExist, FolderNotFound, InternalError, UnknownNode};
use crate::services::dht_map::error::KeyError;
use crate::services::file_storage::errors::*;

#[derive(Debug)]
pub enum ApiException {
    FileNotFound,
    FolderNotFound,
    FileAlreadyExist,
    FolderAlreadyExist,
    BadRequest,
    InternalError(String),
    UnknownNode(String)
}

// #[derive(Error, Debug)]
// pub enum ApiException {
//     #[error(": {0}")]
//     FileNotExist(#[from] FileReadingError::NotExist),
//     #[error("Invalid Id encoding: {0}")]
//     InvalidKey(#[from] KeyError),
//     #[error("An unexpected value was received from dht")]
//     InternalError
// }


impl Display for ApiException {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self, f)
    }
}

impl ResponseError for ApiException {

}

impl From<FolderReadingError> for ApiException {
    fn from(value: FolderReadingError) -> Self {
        match value {
            FolderReadingError::NotExist => FolderNotFound,
            FolderReadingError::InternalError(err) => InternalError(err)
        }
    }
}

impl From<FileReadingError> for ApiException {
    fn from(value: FileReadingError) -> Self {
        match value {
            FileReadingError::NotExist => FileNotFound,
            FileReadingError::BadRequest => BadRequest,
            FileReadingError::ChunkIsNotExist(_) => FileNotFound,
            FileReadingError::Retryable => InternalError("repeat later".to_string())
        }
    }
}

impl From<CreateFolderError> for ApiException {
    fn from(value: CreateFolderError) -> Self {
        match value { CreateFolderError::AlreadyExist => FolderAlreadyExist }
    }
}

impl From<FileSavingError> for ApiException {
    fn from(value: FileSavingError) -> Self {
        match value { FileSavingError::AlreadyExist => FileAlreadyExist }
    }
}

impl From<NodeMetaReceivingError> for ApiException {
    fn from(value: NodeMetaReceivingError) -> Self {
        match value { NodeMetaReceivingError::NotFound => {UnknownNode("Not found".into())} }
    }
}