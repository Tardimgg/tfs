use std::cmp::Ordering;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use typed_builder::TypedBuilder;


#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ChunkVersion(pub u64);

// impl Eq for ChunkVersion {}
//
// impl PartialEq<Self> for ChunkVersion {
//     fn eq(&self, other: &Self) -> bool {
//         self.0.eq(&other.0)
//     }
// }
//
// impl PartialOrd<Self> for ChunkVersion {
//     fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
//         self.0.partial_cmp(&other.0)
//     }
// }
//
// impl Ord for ChunkVersion {
//     fn cmp(&self, other: &Self) -> Ordering {
//         self.partial_cmp(other).unwrap()
//     }
// }


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

#[derive(Serialize, Deserialize, Debug)]
pub enum NodeType {
    File,
    Folder
}

#[derive(Serialize, Deserialize, TypedBuilder, Debug)]
pub struct FileMeta {
    pub name: String,
    pub node_type: NodeType
}


#[derive(Serialize, Deserialize, TypedBuilder, Debug)]
pub struct FolderMeta {
    pub name: String,
    // #[builder(default=vec![])]
    pub files: Vec<FileMeta>
}


#[derive(TypedBuilder)]
pub struct FileSaveOptions {
    #[builder(default=false)]
    pub update_if_exists: bool,
    pub version: ChunkVersion,
}
