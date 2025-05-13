use std::net::IpAddr;
use actix_web::http::header::ByteRangeSpec;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::common::models::UserInfo;
use crate::services::dht_map::model::DhtNodeId;
use crate::services::rebac::models::Permission;

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq, Hash)]
pub enum StoredFileRangeEnd {
    EndOfRange(u64),
    EnfOfFile(u64)
}

impl StoredFileRangeEnd {

    pub fn get_index(&self) -> u64 {
        match self {
            StoredFileRangeEnd::EndOfRange(v) => *v,
            StoredFileRangeEnd::EnfOfFile(v) => *v
        }
    }

}

// нужно перенести в common
#[derive(Debug, Serialize, Deserialize, TypedBuilder, Copy, Clone, PartialEq, Eq, Hash)]
pub struct StoredFile {
    // pub from: u64,
    // pub to: StoredFileRangeEnd,
    pub version: u64, // возможно сходить в узел и проверить как он поживает и что на самом деле хранит имеет смысл
    pub is_keeper: bool
}

/*
impl From<StoredFileRange> for FileRange {
    fn from(value: StoredFileRange) -> Self {
        let end = match value.to {
            StoredFileRangeEnd::EndOfRange(last) => EndOfFileRange::LastByte,
            StoredFileRangeEnd::EnfOfFile(last) => EndOfFileRange::ByteIndex(last)
        };
        (value.from, end)
    }
}

 */

#[derive(Debug, Serialize, Deserialize, TypedBuilder, PartialEq, Eq, Clone)]
pub struct FileKeeper {
    pub id: DhtNodeId,
    pub data: Vec<StoredFile>
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder, Clone)]
pub struct GlobalFileInfo {
    pub filename: String,
    pub file_type: FsNodeType,
    pub id: u64,
    pub keepers: Vec<FileKeeper>
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq)]
pub enum FsNodeType {
    Folder,
    File,
    DataMeta,
    Data,
    Rebac,
    User,

}

#[derive(Debug, Serialize, Deserialize)]
pub enum FsNode {
    Folder(Folder),
    File(File),
    UserInfo(UserInfo),
    RebacPermission(Permission)
}

impl FsNode {
    pub fn get_path(&self) -> Option<&str> {
        match self {
            FsNode::Folder(folder) => Some(&folder.path),
            FsNode::File(file) => Some(&file.path),
            _ => None
        }
    }

    pub fn get_type(&self) -> FsNodeType {
        match self {
            FsNode::Folder(_) => FsNodeType::Folder,
            FsNode::File(_) => FsNodeType::File,
            FsNode::UserInfo(_) => FsNodeType::User,
            FsNode::RebacPermission(_) => FsNodeType::Rebac
        }
    }
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder, Clone)]
pub struct Folder {
    pub path: String,
    #[builder(default=vec![])]
    pub refs: Vec<String>,
    pub id: u64
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct File {
    pub path: String,
    pub data: Vec<((u64, u64), DataMetaKey)>,
    pub id: u64
}

#[derive(Debug, Serialize, Deserialize)]
pub enum DataMetaType {
    Leaf,
    Parent(Vec<((u64, u64), DataMetaKey)>)
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder, Clone)]
pub struct DataMetaKey {
    pub hash: String,
    pub hash_local_id: String
}

impl DataMetaKey {

    pub fn filename(&self) -> String {
        format!("{}_{}", self.hash, self.hash_local_id)
    }


}

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct DataMeta {
    pub key: DataMetaKey,
    pub data_type: DataMetaType
}

// перенести в папку file_storage


// #[derive(Debug)]
// pub enum FileRange {
//     FromTo(u64, u64),
//     From(u64),
//     Last(u64)
// }

// impl From<ByteRangeSpec> for FileRange {
//     fn from(value: ByteRangeSpec) -> Self {
//         match value {
//             ByteRangeSpec::FromTo(from, to) => FileRange::FromTo(from, to),
//             ByteRangeSpec::From(from) => FileRange::From(from),
//             ByteRangeSpec::Last(to) => FileRange::Last(to),
//         }
//     }
// }

