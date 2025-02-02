use std::cmp::Ordering;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use typed_builder::TypedBuilder;

#[derive(Copy, Clone, Debug, Hash)]
pub enum EndOfFileRange {
    LastByte,
    ByteIndex(u64)
}

impl Eq for EndOfFileRange {}

impl PartialEq<Self> for EndOfFileRange {
    fn eq(&self, other: &Self) -> bool {
        self.partial_cmp(other).unwrap().is_eq()
    }
}

impl PartialOrd<Self> for EndOfFileRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self {
            EndOfFileRange::LastByte => {
                if let EndOfFileRange::LastByte = other {
                    Some(Ordering::Equal)
                } else {
                    Some(Ordering::Greater)
                }
            }
            EndOfFileRange::ByteIndex(end) => {
                if let EndOfFileRange::ByteIndex(other_end) = other {
                    Some(end.cmp(other_end))
                } else {
                    Some(Ordering::Less)
                }
            }
        }
    }
}

impl Ord for EndOfFileRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}

pub type FileRange = (u64, EndOfFileRange);

#[derive(Debug, Copy, Clone, Hash)]
pub struct ChunkVersion(pub u64);

impl Eq for ChunkVersion {}

impl PartialEq<Self> for ChunkVersion {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl PartialOrd<Self> for ChunkVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl Ord for ChunkVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}


#[derive(Error, Debug)]
pub enum ChunkVersionParseError {
    #[error("Not a number")]
    NotANumber
}


impl TryFrom<&str> for ChunkVersion {
    type Error = ChunkVersionParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse::<u64>().map(|v| ChunkVersion(v)).or(Err(ChunkVersionParseError::NotANumber))
    }
}

#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct FileMeta {
    name: String
}


#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct FolderMeta {

    // #[builder(default=vec![])]
    files: Vec<FileMeta>
}
