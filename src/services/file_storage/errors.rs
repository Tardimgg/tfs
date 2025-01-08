use thiserror::Error;

// #[derive(Debug)]
pub enum FileSavingError {
    AlreadyExist
}
// #[derive(Error, Debug)]
pub enum FileReadingError {
    NotExist
}

pub enum FolderReadingError {
    NotExist,
    InternalError(String)
}

pub enum CreateFolderError {
    AlreadyExist
}

