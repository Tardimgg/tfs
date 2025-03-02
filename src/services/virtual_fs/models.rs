use std::net::IpAddr;
use actix_web::http::header::ByteRangeSpec;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::services::dht_map::model::DhtNodeId;

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

#[derive(Debug, Serialize, Deserialize, TypedBuilder, Copy, Clone, PartialEq, Eq, Hash)]
pub struct StoredFileRange {
    pub from: u64,
    pub to: StoredFileRangeEnd,
    pub version: u64 // возможно сходить в узел и проверить как он поживает и что на самом деле хранит имеет смысл
}

impl From<StoredFileRange> for FileRange {
    fn from(value: StoredFileRange) -> Self {
        let end = match value.to {
            StoredFileRangeEnd::EndOfRange(last) => EndOfFileRange::LastByte,
            StoredFileRangeEnd::EnfOfFile(last) => EndOfFileRange::ByteIndex(last)
        };
        (value.from, end)
    }
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder, PartialEq, Eq)]
pub struct FileKeeper {
    pub id: DhtNodeId,
    pub ranges: Vec<StoredFileRange>
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct GlobalFileInfo {
    pub filename: String,
    pub keepers: Vec<FileKeeper>
}

// придумать как синхронизировтаь папки (и отделять папки от файлов)

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

