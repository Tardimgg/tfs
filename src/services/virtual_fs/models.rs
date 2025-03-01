use std::net::IpAddr;
use actix_web::http::header::ByteRangeSpec;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;
use crate::services::dht_map::model::DhtNodeId;

#[derive(Debug, Serialize, Deserialize)]
pub enum FileRangeEnd {
    EndOfRange(u64),
    EnfOfFile(u64)
}

impl FileRangeEnd {

    pub fn get_index(&self) -> u64 {
        match self {
            FileRangeEnd::EndOfRange(v) => *v,
            FileRangeEnd::EnfOfFile(v) => *v
        }
    }

}
pub type FileRangeInfo = (u64, FileRangeEnd);

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct FileKeeper {
    pub ip: DhtNodeId,
    pub ranges: Vec<FileRangeInfo>
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct GlobalFileInfo {
    pub filename: String,
    pub keepers: Vec<FileKeeper>
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

