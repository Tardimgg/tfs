use thiserror::Error;
use crate::services::file_storage::model::FileRange;

// #[derive(Debug)]
pub enum FileSavingError {
    AlreadyExist
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



pub enum FolderReadingError {
    NotExist,
    InternalError(String)
}

pub enum CreateFolderError {
    AlreadyExist
}

