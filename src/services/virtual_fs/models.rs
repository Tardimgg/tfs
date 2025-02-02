use std::net::IpAddr;
use actix_web::http::header::ByteRangeSpec;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;
use crate::services::dht_map::model::DhtNodeId;

type FileRangeInfo = (u64, u64);

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct FileKeeper {
    ip: DhtNodeId,
    ranges: Vec<FileRangeInfo>
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct GlobalFileInfo {
    filename: String,
    keepers: Vec<FileKeeper>
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

