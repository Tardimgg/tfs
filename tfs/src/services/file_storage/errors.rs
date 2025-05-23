use std::fmt::{Display, Formatter};
use thiserror::Error;
use crate::common::default_error::DefaultError;
use crate::common::file_range::FileRange;

// #[derive(Debug)]
#[derive(Error, Debug)]
pub enum FileSavingError {
    #[error("Already Exist")]
    AlreadyExist,
    #[error("Other error: {0}")]
    Other(String),
    #[error("Invalid range")]
    InvalidRange,
    #[error("access denied: {0}")]
    AccessDenied(String)
}

#[derive(Error, Debug)]
pub enum FileDeletingError {
    #[error("Not Exist")]
    NotExist,
    #[error("Other error: {0}")]
    Other(String),
    #[error("access denied: {0}")]
    AccessDenied(String)
}

pub enum ChunkSavingExistingError {
    NotExist,
    SavingError(FileSavingError),
    Other(String),
    AccessDenied
}

impl From<String> for ChunkSavingExistingError {
    fn from(value: String) -> Self {
        ChunkSavingExistingError::Other(value)
    }
}

impl From<FileSavingError> for ChunkSavingExistingError {
    fn from(value: FileSavingError) -> Self {
        ChunkSavingExistingError::SavingError(value)
    }
}

impl From<String> for FileSavingError {
    fn from(value: String) -> Self {
        FileSavingError::Other(value)
    }
}

impl From<String> for FileDeletingError {
    fn from(value: String) -> Self {
        FileDeletingError::Other(value)
    }
}

// impl Display for FileSavingError {
//     fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
//         f.write_fmt(format!("{:?}", self))
//     }
// }

#[derive(Error, Debug)]
pub enum FileReadingError {
    #[error("file does not exist")]
    NotExist,
    #[error("bad request")]
    BadRequest,
    #[error("chunk does not exist")]
    ChunkIsNotExist(Vec<FileRange>),
    #[error("internal error, try again later; {0}")]
    Retryable(String),
    #[error("internal error; {0}")]
    InternalError(String),
    #[error("access denied")]
    AccessDenied

}

#[derive(Error, Debug)]
pub enum FolderReadingError {
    #[error("folder does not exist: {0}")]
    NotExist(String),
    #[error("internal error {0}")]
    InternalError(String),
    #[error("bad request {0}")]
    BadRequest(String),
    #[error("access denied")]
    AccessDenied
}

impl From<String> for FolderReadingError {
    fn from(value: String) -> Self {
        FolderReadingError::InternalError(value)
    }
}

impl From<String> for FileReadingError {
    fn from(value: String) -> Self {
        FileReadingError::InternalError(value)
    }
}

#[derive(Error, Debug)]
pub enum CreateFolderError {
    #[error("folder already exist")]
    AlreadyExist,
    #[error("internal error {0}")]
    InternalError(String),
    #[error("access denied: {0}")]
    AccessDenied(String)
}

impl From<String> for CreateFolderError {
    fn from(value: String) -> Self {
        CreateFolderError::InternalError(value)
    }
}
