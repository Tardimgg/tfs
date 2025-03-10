use thiserror::Error;
use crate::common::file_range::FileRange;

// #[derive(Debug)]
pub enum FileSavingError {
    AlreadyExist,
    Other(String),
    InvalidRange
}

pub enum ChunkSavingExistingError {
    NotExist,
    SavingError(FileSavingError),
    Other(String)
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

#[derive(Debug)]
pub enum FileReadingError {
    NotExist,
    BadRequest,
    ChunkIsNotExist(Vec<FileRange>),
    Retryable
}

#[derive(Debug)]
pub enum NodeMetaReceivingError {
    NotFound
}



#[derive(Debug)]
pub enum FolderReadingError {
    NotExist,
    InternalError(String)
}

pub enum CreateFolderError {
    AlreadyExist
}

