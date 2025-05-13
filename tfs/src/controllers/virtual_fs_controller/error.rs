use std::fmt::{Debug, Display, Formatter};
use actix_web::ResponseError;
use thiserror::Error;
use crate::common::exceptions::{ApiException, NodeMetaReceivingError};
use crate::common::exceptions::ApiException::{BadRequest, FileAlreadyExist, FileNotFound, FolderAlreadyExist, FolderNotFound, Forbidden, InternalError, UnknownNode};
use crate::services::dht_map::error::KeyError;
use crate::services::file_storage::errors::*;

// #[derive(Error, Debug)]
// pub enum ApiException {
//     #[error(": {0}")]
//     FileNotExist(#[from] FileReadingError::NotExist),
//     #[error("Invalid Id encoding: {0}")]
//     InvalidKey(#[from] KeyError),
//     #[error("An unexpected value was received from dht")]
//     InternalError
// }




impl From<FolderReadingError> for ApiException {
    fn from(value: FolderReadingError) -> Self {
        match value {
            FolderReadingError::NotExist(_) => FolderNotFound,
            FolderReadingError::InternalError(err) => InternalError(err),
            FolderReadingError::BadRequest(v) => BadRequest(v),
            FolderReadingError::AccessDenied => Forbidden("access denied".to_string())
        }
    }
}

impl From<FileReadingError> for ApiException {
    fn from(value: FileReadingError) -> Self {
        match value {
            FileReadingError::NotExist => FileNotFound,
            FileReadingError::BadRequest => BadRequest("".to_string()),
            FileReadingError::ChunkIsNotExist(_) => FileNotFound,
            FileReadingError::Retryable(v) => InternalError(format!("repeat later: {} ", v)),
            FileReadingError::InternalError(v) => InternalError(v),
            FileReadingError::AccessDenied => Forbidden("access denied".to_string())
        }
    }
}

impl From<CreateFolderError> for ApiException {
    fn from(value: CreateFolderError) -> Self {
        match value {
            CreateFolderError::AlreadyExist => FolderAlreadyExist,
            CreateFolderError::InternalError(v) => InternalError(v),
            CreateFolderError::AccessDenied(v) => Forbidden(v)
        }
    }
}

impl From<FileSavingError> for ApiException {
    fn from(value: FileSavingError) -> Self {
        match value {
            FileSavingError::AlreadyExist => FileAlreadyExist,
            FileSavingError::Other(v) => InternalError(v),
            FileSavingError::InvalidRange => BadRequest("invalid range".to_string()),
            FileSavingError::AccessDenied(v) => Forbidden(v)
        }
    }
}

impl From<ChunkSavingExistingError> for ApiException {
    fn from(value: ChunkSavingExistingError) -> Self {
        match value {
            ChunkSavingExistingError::NotExist => FileNotFound,
            ChunkSavingExistingError::SavingError(v) => v.into(),
            ChunkSavingExistingError::Other(v) => InternalError(v),
            ChunkSavingExistingError::AccessDenied => Forbidden("access denied".to_string())
        }
    }
}

impl From<NodeMetaReceivingError> for ApiException {
    fn from(value: NodeMetaReceivingError) -> Self {
        match value {
            NodeMetaReceivingError::NotFound => UnknownNode("Not found".into()),
            NodeMetaReceivingError::InternalError(v) => InternalError(v),
            NodeMetaReceivingError::AccessDenied => Forbidden("access denied".to_string())
        }
    }
}