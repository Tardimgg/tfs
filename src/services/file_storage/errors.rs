use thiserror::Error;
use crate::common::file_range::FileRange;

// #[derive(Debug)]
pub enum FileSavingError {
    AlreadyExist,
    Other(String)
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

