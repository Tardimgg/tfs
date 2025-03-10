use std::any::Any;
use std::collections::HashSet;
use std::error::Error;
use std::fmt::Debug;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use actix_web::body::MessageBody;
use actix_web::http::header::{ByteRangeSpec, Range};
use actix_web::web;
use bytes::Bytes;
use futures::{FutureExt, Stream, StreamExt, TryStreamExt};
use serde::Serialize;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc::Sender;
use tokio_util::io::ReaderStream;
use tryhard::retry_fn;
use typed_builder::TypedBuilder;
use crate::common::buffered_consumer::BufferedConsumer;
use crate::common::default_error::DefaultError;
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::error::{DhtGetError, DhtPutError};
use crate::services::dht_map::model::{DhtNodeId, PrevSeq};
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

    pub async fn init(&self) -> Result<(), String> {
        let location_prefix = self.config.get_val(ConfigKey::LocationOfKeepersIps).await;

        retry_fn(|| async {
            let mut new_keepers;
            let mut prev_seq = None;
            if let Ok(Some((current_keepers, record_seq))) = self.dht_map.get(&location_prefix).await {
                let mut keepers = serde_json::from_str::<Vec<DhtNodeId>>(&current_keepers).unwrap();
                if !keepers.contains(&self.id) {
                    keepers.push(self.id);
                } else {
                    return Ok(());
                }

                prev_seq = Some(record_seq);
                new_keepers = keepers;
            } else {
                new_keepers = vec![self.id];
            }

            let new_keepers_str = serde_json::to_string(&new_keepers).unwrap();
            let res = self.dht_map.put_with_seq(&location_prefix, &new_keepers_str, PrevSeq::Seq(prev_seq)).await;
            println!("success init {:?}", res);
            return res;
        }).retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 30))
            .await
            .default_res()
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
        let keepers: GlobalFileInfo = serde_json::from_str(&dht_info.unwrap().0).unwrap();

        Ok(keepers)
    }

    pub async fn get_all_keepers(&self) -> Result<Vec<DhtNodeId>, String> {
        let prefix = self.config.get_val(ConfigKey::LocationOfKeepersIps).await;

        if let Ok(Some((keepers, seq))) = self.dht_map.get(&prefix).await {
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

    pub async fn update_state(&self, path: &str, to_add: &HashSet<StoredFileRange>, to_delete: &HashSet<StoredFileRange>, prev_seq: PrevSeq) -> Result<(), String> {
        let prev_o = self.dht_map.get(path).await.default_res()?;

        if let Some(prev) = prev_o {
            let mut info = serde_json::from_str::<GlobalFileInfo>(&prev.0).default_res()?;

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
            self.dht_map.put_with_seq(path, &new_string, prev_seq).await.default_res()?;
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
            self.dht_map.put_with_seq(path, &serde_json::to_string(&file_info).unwrap(), prev_seq).await.default_res()?;
        }
        Ok(())
    }

    pub async fn save_existing_chunk(&self, path: &str, mut data: web::Payload, range_o: Option<Range>, version: u64) -> Result<FileMeta, ChunkSavingExistingError> {
        let mut file_range = parse_range(range_o)?;
        if file_range.len() > 1 {
            return Err(FileSavingError::InvalidRange.into());
        }
        let file_range = file_range.swap_remove(0)?;

        // нужно проверить что не загружаем то что уже есть
        let size = self.storage.save(path, file_range, ChunkVersion(version), FileStream::Payload(data.into())).await?;
        self.state_updater.send(StateUpdate::NewFile(path.to_string())).await.err().iter()
            .for_each(|v| println!("failed to inform file storage about a new file. Err = {}", v));

        let range_info = StoredFileRange {
            from: file_range.0,
            to: StoredFileRangeEnd::EnfOfFile(file_range.0 + size),
            version,
            is_keeper: true
        };

        let mut new = HashSet::new();
        new.insert(range_info);
        self.update_state(path, &new, &HashSet::new(), PrevSeq::Any).await.err().iter()
            .for_each(|v| println!("information about the new file could not be saved to dht. Err = {}", v));
        Ok(FileMeta::builder().name(path.to_string()).node_type(NodeType::File).build())
    }

    pub async fn put_file(&self, path: &str, mut data: web::Payload, range_o: Option<Range>) -> Result<FileMeta, FileSavingError> {

        // заборы про пути строим тут

        let mut file_range = parse_range(range_o)?;
        if file_range.len() > 1 {
            return Err(FileSavingError::InvalidRange);
        }
        let file_range = file_range.swap_remove(0)?;

        let locked_version = retry_fn(|| async {
            let already_exist_o = self.dht_map.get(path).await.default_res()?;
            let prev_seq;
            let version = if let Some(already_exist) = already_exist_o {
                prev_seq = Some(already_exist.1);
                let file_meta = parse_dht_val(&already_exist.0)?;
                file_meta.keepers.iter().flat_map(|v| v.ranges.iter().map(|v| v.version + 1).max()).max().unwrap_or(0)
            } else {
                prev_seq = None;
                0
            };

            let lock_version_range = StoredFileRange {
                from: 0,
                to: StoredFileRangeEnd::EndOfRange(0),
                version,
                is_keeper: false
            };
            let mut new = HashSet::new();
            new.insert(lock_version_range);
            self.update_state(path, &new, &HashSet::new(), PrevSeq::Seq(prev_seq)).await
                .map_err(|v| format!("information about the lock version file could not be saved to dht. Err = {}", v))?;

            Ok::<u64, String>(version)
        })
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(3), 30))
            .await?;


        // нужно проверить что не загружаем то что уже есть
        let size = self.storage.save(path, file_range, ChunkVersion(locked_version), FileStream::Payload(data.into())).await?;
        self.state_updater.send(StateUpdate::NewFile(path.to_string())).await.err().iter()
            .for_each(|v| println!("failed to inform file storage about a new file. Err = {}", v));

        let range_info = StoredFileRange {
            from: file_range.0,
            to: StoredFileRangeEnd::EnfOfFile(file_range.0 + size),
            version: locked_version,
            is_keeper: true
        };

        let mut new = HashSet::new();
        new.insert(range_info);
        let mut lock = HashSet::new();
        lock.insert(StoredFileRange {
            from: 0,
            to: StoredFileRangeEnd::EndOfRange(0),
            version: locked_version,
            is_keeper: false
        });
        self.update_state(path, &new, &lock, PrevSeq::Any).await.err().iter()
            .for_each(|v| println!("information about the new file could not be saved to dht. Err = {}", v));
        Ok(FileMeta::builder().name(path.to_string()).node_type(NodeType::File).build())
    }
}

fn parse_dht_val(val: &str) -> Result<GlobalFileInfo, String> {
    Ok(serde_json::from_str::<GlobalFileInfo>(val).default_res()?.into())
}

fn parse_range(range_o: Option<Range>) -> Result<Vec<Result<FileRange, String>>, String> {
    let mut validated_range = None;
    if let Some(range) = range_o {
        if let Range::Bytes(range_bytes) = range {
            validated_range = Some(range_bytes)
        } else {
            return Err("invalid range".to_string())
        }
    };
    // заборы про пути строим тут

    let file_ranges = if let Some(ranges) = validated_range {
        ranges.into_iter().map(|range| {
            match range {
                ByteRangeSpec::FromTo(from, to) => Ok((from, EndOfFileRange::ByteIndex(to))),
                ByteRangeSpec::From(from) => Ok((from, EndOfFileRange::LastByte)),
                ByteRangeSpec::Last(_) => Err("invalid range start".to_string()),
            }
        }).collect()
    } else {
        vec![Ok((0, EndOfFileRange::LastByte))]
    };
    Ok(file_ranges)
}