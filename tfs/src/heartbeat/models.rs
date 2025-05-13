use thiserror::Error;
use crate::common::file_range::EndOfFileRange;
use crate::services::virtual_fs::models::StoredFileRangeEnd;


#[derive(Copy, Clone)]
pub struct RangeEvent {
    pub chunk_version: u64,
    pub event: RangeEventType
}

impl RangeEvent {
    pub fn new(chunk_version: u64, event: RangeEventType) -> Self {
        Self { chunk_version, event }
    }
}

#[derive(Copy, Clone)]
pub enum RangeEventType {
    Start(u64),
    End(u64),
    StoredStart(u64, usize),
    StoredEnd(StoredFileRangeEnd, usize)
}

#[derive(Error, Debug)]
pub enum ReplicateFileError {
    #[error("internal error: {0}")]
    InternalError(String),
    #[error("no new keeper was found for versions")]
    NotFoundNewKeeper(Vec<u64>)
}

pub enum ReplicateFileStatus {
    Success,
    NotMaster
}

impl From<String> for ReplicateFileError {
    fn from(value: String) -> Self {
        ReplicateFileError::InternalError(value)
    }
}

impl RangeEventType {

    pub fn get_index(&self) -> (u64, u8) {
        match self {
            RangeEventType::Start(v) => (*v, 0),
            RangeEventType::End(v) => (*v, 3),
            RangeEventType::StoredStart(v, _) => (*v, 1),
            RangeEventType::StoredEnd(v, _) => {
                match v {
                    StoredFileRangeEnd::EnfOfFile(v) => (*v, 2),
                    StoredFileRangeEnd::EndOfRange(v) => (*v, 2)
                }
            }
        }
    }
}