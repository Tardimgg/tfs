use std::any::Any;
use std::collections::HashSet;
use std::error::Error;
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
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::error::{DhtGetError, DhtPutError};
use crate::services::dht_map::model::DhtNodeId;
use crate::services::file_storage::errors::*;
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::file_storage::model::{ChunkVersion, FileMeta, FolderMeta, NodeType};
use crate::services::virtual_fs::models::{FileKeeper, StoredFileRangeEnd, StoredFileRange, GlobalFileInfo};

#[derive(TypedBuilder)]
pub struct VirtualFS { // все эти поля это конфиг, нужно отделить данные от поведения
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

    pub async fn init(&self) {
        let location_prefix = self.config.get_val(ConfigKey::LocationOfKeepersIps).await;

        let mut new_keepers;
        if let Ok(Some(current_keepers)) = self.dht_map.get(&location_prefix).await {
            let mut keepers = serde_json::from_str::<Vec<DhtNodeId>>(&current_keepers).unwrap();
            if !keepers.contains(&self.id) {
                keepers.push(self.id);
            }

            new_keepers = keepers;
            println!("success init")
        } else {
            new_keepers = vec![self.id];
        }

        let new_keepers_str = serde_json::to_string(&new_keepers).unwrap();
        let res = self.dht_map.put(&location_prefix, &new_keepers_str).await;
        println!("success init {:?}", res)
    }

    pub fn get_id(&self) -> &DhtNodeId {
        &self.id
    }

    pub async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError> {
        self.storage.get_folder_content(path).await

        // переделать на схему врапперов итераторов  VirtualFS() // нинада
    }

    pub async fn get_stored_parts_meta(&self, path: &str) -> Result<Vec<StoredFileRange>, String> {
        self.storage.get_file_meta(path).await.map_err(|v| format!("{:?}", v))
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

    pub async fn update_state(&self, path: &str, to_add: &HashSet<StoredFileRange>, to_delete: &HashSet<StoredFileRange>) -> Result<(), String> {
        let prev_o = self.dht_map.get(path).await.map_err(|v| v.to_string())?;

        if let Some(prev) = prev_o {
            let mut info = serde_json::from_str::<GlobalFileInfo>(&prev).map_err(|v| v.to_string())?;

            let mut first_record = true;
            for keeper in &mut info.keepers {
                let mut to_delete_i = Vec::new();
                let mut to_add_skipped = HashSet::new();

                if keeper.id == self.id {
                    first_record = false;
                    for i in 0..keeper.ranges.len() {
                        let chunk = keeper.ranges[i];

                        if to_add.contains(&chunk) {
                            to_add_skipped.insert(chunk);
                            continue;
                        }
                        if to_delete.contains(&chunk) {
                            to_delete_i.push(i);
                        }
                    }

                    for i in to_delete_i.iter().rev() {
                        keeper.ranges.swap_remove(*i);
                    }

                    for new_chunk in to_add {
                        if to_add_skipped.contains(new_chunk) {
                            continue;
                        }
                        keeper.ranges.push(*new_chunk);
                    }
                }
            }
            if first_record {
                let keeper = FileKeeper {
                    id: self.id,
                    ranges: to_add.iter().map(|v| *v).collect(),
                };
                info.keepers.push(keeper);
            }

            let new_string = serde_json::to_string(&info).unwrap();
            println!("new dht path {} val {} ", path, new_string);
            self.dht_map.put(path, &new_string).await.unwrap();
        } else {
            let keeper = FileKeeper::builder()
                .id(self.id)
                .ranges(to_add.iter().map(|v| *v).collect())
                .build();

            //что делать со знанием, что actix web не меняет поток в течении ответа на запрос (не требуется Send на future)
            let file_info = GlobalFileInfo::builder()
                .filename(path.to_string())
                .keepers(vec![keeper])
                .build();
            self.dht_map.put(path, &serde_json::to_string(&file_info).unwrap()).await.unwrap();
        }
        Ok(())
    }

    pub async fn create_file(&self, path: &str, mut data: web::Payload) -> Result<FileMeta, FileSavingError> {
        // заборы про пути строим тут

        self.storage.save(path, (0, EndOfFileRange::LastByte), ChunkVersion(0), FileStream::Payload(data.into())).await?;
        self.state_updater.send(StateUpdate::NewFile(path.to_string())).await.unwrap();

        let range_info = StoredFileRange {
            from: 0,
            to: StoredFileRangeEnd::EnfOfFile(u64::MAX),
            version: 1,
        };

        let mut new = HashSet::new();
        new.insert(range_info);
        self.update_state(path, &new, &HashSet::new()).await.map_err(|v| FileSavingError::Other(v))?;
        // let keeper = FileKeeper::builder()
        //     .id(self.id)
        //     .ranges(vec![range_info])
        //     .build();
        //
        // //что делать со знанием, что actix web не меняет поток в течении ответа на запрос (не требуется Send на future)
        // let file_info = GlobalFileInfo::builder()
        //     .filename(path.to_string())
        //     .keepers(vec![keeper])
        //     .build();
        //
        // self.dht_map.put(path, &serde_json::to_string(&file_info).unwrap()).await.unwrap();
        Ok(FileMeta::builder().name(path.to_string()).node_type(NodeType::File).build())
    }

}