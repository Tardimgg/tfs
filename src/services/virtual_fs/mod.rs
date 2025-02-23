pub mod models;

use std::any::Any;
use std::fmt::Debug;
use std::net::IpAddr;
use std::sync::Arc;
use actix_web::body::MessageBody;
use actix_web::http::header::{ByteRangeSpec, Range};
use actix_web::web;
use bytes::Bytes;
use futures::{FutureExt, Stream, StreamExt, TryStreamExt};
use serde::Serialize;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc::Sender;
use tokio_util::io::ReaderStream;
use typed_builder::TypedBuilder;
use crate::common::buffered_consumer::BufferedConsumer;
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::error::{DhtGetError, DhtPutError};
use crate::services::dht_map::model::DhtNodeId;
use crate::services::file_storage::errors::*;
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::file_storage::model::{ChunkVersion, EndOfFileRange, FileMeta, FileRange, FolderMeta};
use crate::services::virtual_fs::models::{FileKeeper, GlobalFileInfo};

#[derive(TypedBuilder)]
pub struct VirtualFS {
    dht_map: Arc<DHTMap>,
    storage: Arc<dyn FileStorage + Send + Sync>, // поменять на дженерик, для решения во время компиляции. Попробовать внтури impl вместо Box<dyn>
    // при передаче стрима с файлом

    #[builder(default="root".to_string())]
    base_path: String,
    id: DhtNodeId,
    config: Arc<GlobalConfig>,
    state_updater: Sender<StateUpdate>
}

impl VirtualFS {

    pub async fn new(map: Arc<DHTMap>, id: DhtNodeId, storage: Arc<dyn FileStorage + Send + Sync>, config: Arc<GlobalConfig>, state_updater: Sender<StateUpdate>) -> Self {
        let instance = VirtualFS {
            dht_map: map, storage,
            base_path: "root".to_string(),
            id,
            config,
            state_updater
        };
        instance.init().await;
        instance
    }

    async fn init(&self) {
        let str_id = serde_json::to_string(&self.id).unwrap();
        let location_prefix = self.config.get_val(ConfigKey::LocationOfKeepersIps).await;
        self.dht_map.put(&location_prefix, &str_id).await.unwrap();
    }

    pub async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError> {
        self.storage.get_folder_content(path).await

        // переделать на схему врапперов итераторов  VirtualFS() // нинада
    }

    pub async fn get_node_meta(&self, path: &str) -> Result<GlobalFileInfo, NodeMetaReceivingError> {
        let dht_info = self.dht_map.get(path).await.unwrap();
        if dht_info.is_none() {
            return Err(NodeMetaReceivingError::NotFound);
        }
        let keepers: GlobalFileInfo = serde_json::from_str(&dht_info.unwrap()).unwrap();

        Ok(keepers)
    }

    pub async fn get_all_keepers(&self) -> Result<Vec<DhtNodeId>, String> {
        let prefix = self.config.get_val(ConfigKey::LocationOfKeepersIps).await;

        if let Ok(Some(keepers)) = self.dht_map.get(&prefix).await {
            Ok(serde_json::from_str::<Vec<DhtNodeId>>(&keepers).unwrap().into())
        } else {
            Err("err".to_string())
        }
    }

    pub async fn get_file_content(&self, path: &str, range_o: Option<Range>) -> Result<Option<FileStream>, FileReadingError> {
        let mut validated_range = None;
        if let Some(range) = range_o {
            if let Range::Bytes(range_bytes) = range {
                validated_range = Some(range_bytes)
            } else {
                return Err(FileReadingError::BadRequest)
            }
        };


        let parsed_ranges: Option<Vec<FileRange>> = validated_range.map(
            |ranges| ranges.into_iter().map(|v| match v {
                ByteRangeSpec::FromTo(from, to) => (from, EndOfFileRange::ByteIndex(to)),
                ByteRangeSpec::From(from) => (from, EndOfFileRange::LastByte),
                ByteRangeSpec::Last(last) => panic!()
            }).collect()
        );

        self.storage.get_file(path, parsed_ranges.as_deref(), None).await.map(|mut v| v.swap_remove(0).1).map(|v| Some(v))
    }

    pub async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError> {
        self.storage.create_folder(path).await

    }
    pub async fn create_file(&self, path: &str, mut data: web::Payload) -> Result<FileMeta, FileSavingError> {
        // заборы про пути строим тут

        self.storage.save(path, (0, EndOfFileRange::LastByte), ChunkVersion(0), FileStream::Payload(data.into())).await?;

        let keeper = FileKeeper::builder()
            .ip(self.id)
            .ranges(vec![(0, u64::MAX)])
            .build();

        let file_info = GlobalFileInfo::builder()
            .filename(path.to_string())
            .keepers(vec![keeper])
            .build();

        self.dht_map.put(path, &serde_json::to_string(&file_info).unwrap()).await.unwrap();
        Ok(FileMeta::builder().name(path.to_string()).build())
    }

}