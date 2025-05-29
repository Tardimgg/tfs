use std::any::Any;
use std::cell::RefCell;
use std::collections::HashSet;
use std::error::Error;
use std::fmt::Debug;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use actix_web::body::MessageBody;
use actix_web::http::header::{ByteRangeSpec, Range};
use actix_web::{web, ResponseError};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use futures::future::join_all;
use log::{debug, error, info};
use rand::distributions::Alphanumeric;
use rand::prelude::ThreadRng;
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::pin;
use tokio::runtime::Handle;
use tokio::sync::broadcast::channel;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::io::ReaderStream;
use tryhard::retry_fn;
use typed_builder::TypedBuilder;
use crate::common::buffered_consumer::BufferedConsumer;
use crate::common::common::read_stream_to_string;
use crate::common::default_error::DefaultError;
use crate::common::exceptions::NodeMetaReceivingError;
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::common::models::FolderContent;
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::common::retry_police::linear_backoff_with_jitter::LinearBackoffWithJitter;
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::heartbeat::heartbeat::Heartbeat;
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::error::{DhtGetError, DhtPutError};
use crate::services::dht_map::model::{DhtNodeId, PrevSeq};
use crate::services::file_storage::errors::*;
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::file_storage::model::{ChunkVersion, FileMeta, FolderMeta, NodeType, FileSaveOptions};
use crate::services::internal_communication::InternalCommunication;
use crate::services::virtual_fs::models::{FileKeeper, StoredFileRangeEnd, StoredFile, GlobalFileInfo, File, DataMeta, DataMetaKey, DataMetaType, Folder, FsNode, FsNodeType};

#[derive(TypedBuilder)]
pub struct VirtualFS {
    dht_map: Arc<DHTMap>,
    storage: Arc<dyn FileStorage + Send + Sync>,
    #[builder(default="root".to_string())]
    base_path: String,
    id: DhtNodeId,
    config: Arc<GlobalConfig>,
    state_updater: Sender<StateUpdate>,
    client: InternalCommunication
}

thread_local! {
    static RANDOM: RefCell<ThreadRng> = RefCell::new(rand::thread_rng());
}

impl VirtualFS {

    pub async fn new(map: Arc<DHTMap>, id: DhtNodeId, storage: Arc<dyn FileStorage + Send + Sync>,
                     config: Arc<GlobalConfig>, state_updater: Sender<StateUpdate>) -> Self {
        let instance = VirtualFS {
            dht_map: map,
            storage,
            base_path: "root".to_string(),
            id,
            config: config.clone(),
            state_updater,
            client: InternalCommunication::builder().config(config).build()
        };
        instance.init().await;
        instance
    }

    pub async fn init(&self) -> Result<(), String> {
        let location_prefix = self.config.get_val(ConfigKey::LocationOfKeepersIps).await;

        retry_fn(|| async {
            let mut new_keepers;
            let mut prev_seq = None;
            if let Some((current_keepers, record_seq)) = self.dht_map.get(&location_prefix).await.default_res()? {
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

    pub async fn get_folder_content(&self, path: &str) -> Result<FolderContent, FolderReadingError> {
        let path = path
            .trim_start_matches("/")
            .trim_end_matches("/");

        let folder_json = self.dht_map.get(path).await.default_res()?;
        let folder_json = folder_json.ok_or(FolderReadingError::NotExist(format!("{:?} does not exist", path)))?;

        let mut folder_keepers = serde_json::from_str::<GlobalFileInfo>(&folder_json.0).default_res()?;

        if folder_keepers.file_type != FsNodeType::Folder {
            return Err(FolderReadingError::BadRequest(format!("{:?} is not a folder", path)));
        }

        let keeper_o = folder_keepers.keepers.iter()
            // .filter(|v| v.id != self.id)
            .max_by_key(|v| v.data.iter().max_by_key(|v| v.version).unwrap().version);
        let keeper = keeper_o.ok_or("No one keeps the file with the folder's metadata".to_string())?;

        let folder_stream = self.client.get_file_content(path, keeper.id).await
            .default_logging_res("when get file content: ")?;
        let folder_str = read_stream_to_string(folder_stream.map_err(|v| v.to_string())).await?;

        let mut folder = serde_json::from_str::<FsNode>(&folder_str).default_res()?;

        if let FsNode::Folder(folder) = folder {

            let folder_childs_tasks: Vec<_> = folder.refs.iter().map(|child| async {
                let node_info = self.get_node_meta(child).await;
                return node_info.map(|v| v.file_type).default_res();
            }).collect();

            let folder_childs = join_all(folder_childs_tasks).await;

            Ok(FolderContent::builder()
                .name(Path::new(&folder.path).file_name().unwrap().to_str().unwrap().to_string())
                .files(folder.refs.into_iter().zip(folder_childs).map(|(file, node_info)|
                    FileMeta::builder()
                        .name(Path::new(&file).file_name().unwrap().to_str().unwrap().to_string())
                        .node_type(fs_node_type_to_external(node_info.unwrap_or(FsNodeType::File)))
                        .build()
                ).collect())
                .id(folder.id)
                .build()
            )
        } else {
            Err(FolderReadingError::InternalError(format!("{:?} is not a folder", folder)))
        }

        // переделать на схему врапперов итераторов  VirtualFS() // нинада
    }

    pub async fn get_stored_parts_meta(&self, path: &str) -> Result<Vec<StoredFile>, String> {
        self.storage.get_file_meta(path).await.map_err(|v| format!("{:?}", v))
    }


    pub async fn get_node_meta(&self, path: &str) -> Result<GlobalFileInfo, NodeMetaReceivingError> {
        let dht_info = self.dht_map.get(path).await.map_err(|err| format!("error when get_node_meta, path: {path}, err: {err}"))?;
        if dht_info.is_none() {
            return Err(NodeMetaReceivingError::NotFound);
        }
        let keepers: GlobalFileInfo = serde_json::from_str(&dht_info.unwrap().0).unwrap();

        Ok(keepers)
    }

    pub async fn get_all_keepers(&self) -> Result<Vec<DhtNodeId>, String> {
        let prefix = self.config.get_val(ConfigKey::LocationOfKeepersIps).await;

        match self.dht_map.get(&prefix).await {
            Ok(Some((keepers, seq))) => Ok(serde_json::from_str::<Vec<DhtNodeId>>(&keepers).unwrap().into()),
            Err(err) => Err(err.to_string()),
            Ok(None) => Ok(vec![])
        }
    }

    pub async fn get_distributed_file_content(&self, path: &str) -> Result<Option<FileStream>, FileReadingError> {
        let node_stream = self.get_node_stream(path).await?;

        if let None = node_stream {
            return Ok(None);
        }
        let node_stream = node_stream.unwrap();

        return Ok(Some(node_stream))
        // let json_str = read_stream_to_string(node_stream.get_stream())
        //     .await
        //     .default_logging_res("read file meta stream to string: ")?;

        // let fs_node = serde_json::from_str::<FsNode>(&json_str).default_logging_res("virtual_fs")?;

        // if let FsNode::File(file) = fs_node {
        //     Ok(Some(file))
        // } else {
        //     Err(FileReadingError::BadRequest)
        // }
    }

    pub async fn get_node_stream(&self, path: &str) -> Result<Option<FileStream>, FileReadingError> {
        let json = self.dht_map.get(path).await.default_res()?;
        let json = json.ok_or(FileReadingError::NotExist)?;

        let mut file_info = serde_json::from_str::<GlobalFileInfo>(&json.0).default_res()?;

        let max_version = file_info.keepers.iter()
            .map(|v| v.data.iter().max_by_key(|v| v.version).unwrap().version)
            .max();

        if let None = max_version {
            return Ok(None)
        }
        let max_version = max_version.unwrap();

        if let Ok(mut local_file) = self.storage.get_file(path, None, Some(ChunkVersion(max_version))).await {
            Ok(Some(local_file.swap_remove(0).1))
        } else {
            let keeper_o = file_info.keepers.iter()
                // .filter(|v| v.id != self.id)
                .max_by_key(|v| v.data.iter().max_by_key(|v| v.version).unwrap().version);
            let keeper = keeper_o.ok_or("No one keeps the file with the folder's metadata".to_string())?;

            let stream = self.client.get_file_content(path, keeper.id).await
                .default_logging_res("when get file content: ")?;

            return Ok(Some(FileStream::DownloadStream(Box::new(stream))))
        }
    }

    pub async fn get_file_content(&self, path: &str, range_o: Option<Range>) -> Result<Option<FileStream>, FileReadingError> {
        // сходил в dht, узнал где мета о файле
        // нашел мету о файле, пошел узнавать где находится мета о data
        // нашёл мету о data, пошел искать детей, из которых состоит data
        // добрался до меты, где написан id реальной data
        // пошел в dht узнавать где находится этот реальный id с data
        // получил узлы где есть, скачать оттуда файлик (этот не чанкуется, хотя вроде уже ничего не чанкуется)

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

        self.storage.get_file(path, parsed_ranges.as_deref(), None).await
            .map(|mut v| v.swap_remove(0).1).map(|v| Some(v))
    }

    pub async fn create_folder(&self, path: &str) -> Result<Folder, CreateFolderError> {
        let path = path
            .trim_start_matches("/")
            .trim_end_matches("/");
        if self.check_exist(Path::new(path)).await.default_res()?.is_some() {
            return Err(CreateFolderError::AlreadyExist);
        }
        if !path.starts_with(&self.config.get_val(ConfigKey::HomePath).await) {
            return Err(CreateFolderError::AccessDenied("no rights to write to the specified folder".to_string()))
        }

        let folder_id = self.lock_generated_id(path).await?;

        // check that it doesnt exist

        let folder = Folder {
            path: path.to_string(),
            refs: vec![],
            id: folder_id
        };

        self.save_node(path, &FsNode::Folder(folder.clone()), 0, PrevSeq::Any).await?;

        let mut root_folder = PathBuf::from(path);
        root_folder.pop();

        self.put_file_into_folder(Path::new(path), &root_folder).map_err(|v| CreateFolderError::InternalError(v)).await?;

        Ok(folder)
    }

    pub async fn get_ancestor_folders(&self, obj_id: u64) -> Result<Option<Vec<String>>, String> {
        let filename = self.filename_by_id(obj_id).await?;

        if let None = filename {
            return Ok(None)
        }

        let mut path = PathBuf::from(filename.unwrap());
        let mut ancestors = path.ancestors();
        ancestors.next();

        let mut ans = Vec::new();
        while let Some(ancestor) = ancestors.next() {
            ans.push(ancestor.to_str().unwrap().to_string())
        }

        Ok(Some(ans))
    }

    pub async fn filename_by_id(&self, obj_id: u64) -> Result<Option<String>, String> {
        let path = self.config.get_val(ConfigKey::FsNodeIdMappingPath).await;
        let record = self.dht_map.get(&format!("{}/{}", path, obj_id))
            .await
            .default_res()?;

        if record.is_none() {
            return Ok(None);
        }
        let record = record.unwrap();

        Ok(Some(record.0))
    }

    async fn generate_id(&self) -> Result<u64, String> {
        let mut id;
        let mut attempt_counter = 0;
        loop {
            attempt_counter += 1;
            id = RANDOM.take().gen_range(0..i64::MAX) as u64;
            let filename = self.filename_by_id(id).await;

            if let Ok(exists_filename) = filename {
                if exists_filename.is_none() {
                    return Ok(id);
                }
                if attempt_counter == 3 {
                    return Err("couldn't select a unique id".to_string())
                }
            }
            if attempt_counter == 3 {
                return Err("internal error during id generation".to_string())
            }
        }
    }

    pub async fn lock_id(&self, id: u64, filename: &str) -> Result<(), String> {
        // не будет жить при переименовании файла
        let path = self.config.get_val(ConfigKey::FsNodeIdMappingPath).await;
        self.dht_map.put_with_seq(&format!("{}/{}", path, id), filename, PrevSeq::Seq(None)).await.default_res()?;
        Ok(())
    }

    pub async fn lock_user_uid_mapping(&self, uid: u64, login: &str) -> Result<(), String> {
        // не будет жить при переименовании файла
        let path = self.config.get_val(ConfigKey::UserUidMappingPath).await;
        self.dht_map.put_with_seq(&format!("{}/{}", path, login), &uid.to_string(), PrevSeq::Seq(None)).await.default_res()?;
        Ok(())
    }

    pub async fn uid_by_login(&self, login: &str) -> Result<Option<u64>, String> {
        let path = self.config.get_val(ConfigKey::UserUidMappingPath).await;
        let uid = self.dht_map.get(&format!("{}/{}", path, login)).await.default_res()?;

        if let Some(some_uid) = uid {
            Ok(Some(u64::from_str(&some_uid.0).default_res()?))
        } else {
            Ok(None)
        }
    }

    async fn lock_generated_id(&self, filename: &str) -> Result<u64, String> {
        retry_fn(async || {
            let generated_id = self.generate_id().await?;
            self.lock_id(generated_id, filename).await?;

            Ok(generated_id)
        })
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 30))
            .await
    }

    pub async fn update_state(&self, path: &str, record_id: Option<u64>, file_type: FsNodeType, to_add: &HashSet<StoredFile>,
                              to_delete: &HashSet<StoredFile>, put_with_seq: PrevSeq) -> Result<(), String> {
        // тут удаляем слишком старые версии
        let prev_o = self.dht_map.get(path).await.default_res()?;

        if let Some((prev, prev_seq)) = prev_o {
            let mut info = serde_json::from_str::<GlobalFileInfo>(&prev).default_res()?;

            let max_version = info.keepers.iter()
                .flat_map(|v| &v.data)
                .max_by_key(|v| v.version)
                .map(|v| v.version)
                .unwrap_or(0);

            let max_version_delta = u64::checked_sub(
                u64::from_str(&self.config.get_val(ConfigKey::NumberOfStoredVersions).await).default_res()?,
                1
            ).unwrap_or(0);
            let min_allowed_version = u64::checked_sub(max_version, max_version_delta)
                .unwrap_or(0);

            let mut to_delete_keeper_i = Vec::new();

            let mut first_local_record = true;
            let mut changed = false;
            for keeper_i in 0..info.keepers.len() {
                let keeper = &mut info.keepers[keeper_i];
                let mut to_delete_i = Vec::new();
                let mut to_add_skipped = HashSet::new();

                for i in 0..keeper.data.len() {
                    if keeper.data[i].version < min_allowed_version {
                        to_delete_i.push(i)

                    } else if keeper.id == self.id {
                        first_local_record = false;
                        let chunk = keeper.data[i];

                        if to_add.contains(&chunk) {
                            to_add_skipped.insert(chunk);
                            continue;
                        }
                        if to_delete.contains(&chunk) {
                            to_delete_i.push(i);
                        }
                    }
                }
                if keeper.id == self.id {
                    for new_chunk in to_add {
                        if to_add_skipped.contains(new_chunk) || new_chunk.version < min_allowed_version {
                            continue;
                        }
                        changed = true;
                        keeper.data.push(*new_chunk);
                    }
                }
                if keeper.data.len() == to_delete_i.len() {
                    to_delete_keeper_i.push(keeper_i)
                } else {
                    if !to_delete_i.is_empty() {
                        changed = true;
                    }
                    for i in to_delete_i.iter().rev() {
                        keeper.data.swap_remove(*i);
                    }
                }
            }
            for i in to_delete_keeper_i.iter().rev() {
                changed = true;
                info.keepers.swap_remove(*i);
            }
            if first_local_record {
                let to_add_filtered: Vec<StoredFile> = to_add.clone().into_iter()
                    .filter(|v| v.version >= min_allowed_version)
                    .collect();

                if to_add_filtered.len() > 0 {
                    let keeper = FileKeeper {
                        id: self.id,
                        data: to_add_filtered.into_iter().map(|v| v).collect(),
                    };
                    changed = true;
                    info.keepers.push(keeper);
                }
            }

            if let Some(some_record_id) = record_id {
                if info.id != some_record_id {
                    info.id = some_record_id;
                    changed = true;
                }
            }

            if changed {
                let new_string = serde_json::to_string(&info).unwrap();
                debug!("new dht path {} val {} ", path, new_string);
                self.dht_map.put_with_seq(path, &new_string, put_with_seq).await.default_res()?;
            }
        } else {
            let keeper = FileKeeper::builder()
                .id(self.id)
                .data(to_add.iter().map(|v| *v).collect())
                .build();

            //что делать со знанием, что actix web не меняет поток в течении ответа на запрос (не требуется Send на future)
            let file_info = GlobalFileInfo::builder()
                .filename(path.to_string())
                .keepers(vec![keeper])
                .file_type(file_type)
                .id(record_id.unwrap_or(0))
                .build();
            self.dht_map.put_with_seq(path, &serde_json::to_string(&file_info).unwrap(), put_with_seq).await.default_res()?;
        }
        Ok(())
    }

    pub async fn save_existing_chunk(&self, path: &str, mut data: web::Payload, range_o: Option<Range>,
                                     fs_node_type: FsNodeType, version: u64) -> Result<FileMeta, ChunkSavingExistingError> {
        let mut file_range = parse_range(range_o)?;
        if file_range.len() > 1 {
            return Err(FileSavingError::InvalidRange.into());
        }
        let file_range = file_range.swap_remove(0)?;

        // нужно проверить что не загружаем то что уже есть
        let options = FileSaveOptions::builder()
            .version(ChunkVersion(version))
            .build();
        let size = self.storage.save(path, file_range, FileStream::Payload(data.into()), options).await?;

        self.state_updater.send(new_node_to_state_update(fs_node_type, path.to_string())).await.err().iter()
            .for_each(|v| error!("failed to inform file storage about a new file. Err = {}", v));

        let range_info = StoredFile {
            // from: file_range.0,
            // to: StoredFileRangeEnd::EnfOfFile(file_range.0 + size),
            version,
            is_keeper: true
        };

        let mut new = HashSet::new();
        new.insert(range_info);
        self.update_state(path, None, fs_node_type, &new, &HashSet::new(), PrevSeq::Any).await.err().iter()
            .for_each(|v| error!("information about the new file could not be saved to dht. Err = {}", v));

        let node_type = match fs_node_type {
            FsNodeType::Folder => NodeType::Folder,
            _ => NodeType::File
        };
        Ok(FileMeta::builder().name(path.to_string()).node_type(node_type).build())
    }

    async fn check_exist(&self, path: &Path) -> Result<Option<FsNodeType>, String> {
        let str_path = path.to_str().unwrap();
        let node = self.dht_map.get(str_path).await.default_res()?;

        if let Some(some) = node {
           let parsed = serde_json::from_str::<GlobalFileInfo>(&some.0).default_res()?;
            return Ok(Some(parsed.file_type))
        }
        Ok(None)
    }

    pub async fn lock_next_file_version(&self, path: &str, file_type: FsNodeType) -> Result<(u64, Option<GlobalFileInfo>), String> {
        let already_exist_o = self.dht_map.get(path).await.default_res()?;
        let prev_seq;
        let mut version;
        let mut prev_file_meta;
        if let Some((already_exist, exist_seq)) = already_exist_o {
            let file_meta = parse_dht_val(&already_exist)?;
            if file_meta.file_type != file_type {
                return Err(format!("the existing record is of a different type. Exist {:?}, attempt to update {:?}",
                                   file_meta.file_type, file_type));
            }
            if let Some(v) = file_meta.keepers.iter()
                .flat_map(|v| v.data.iter().max_by_key(|range| range.version))
                .max_by_key(|range| range.version) {

                if v.is_keeper {
                    version = v.version + 1;
                } else {
                    return Err("file is already being updated".to_string());
                }
            } else {
                version = 0;
            }
            prev_file_meta = Some(file_meta);
            prev_seq = Some(exist_seq);
        } else {
            prev_seq = None;
            prev_file_meta = None;
            version = 0;
        };

        let lock_version_range = StoredFile {
            // from: 0,
            // to: StoredFileRangeEnd::EndOfRange(0),
            version,
            is_keeper: false
        };
        let mut new = HashSet::new();
        new.insert(lock_version_range);
        self.update_state(path, prev_file_meta.as_ref().map(|v| v.id), file_type, &new, &HashSet::new(), PrevSeq::Seq(prev_seq)).await
            .map_err(|v| format!("information about the lock version file could not be saved to dht. Err = {}", v))?;

        Ok((version, prev_file_meta))
    }

    pub async fn save_node(&self, path: &str, node: &FsNode, version: u64, prev_seq: PrevSeq) -> Result<(), String> {

        let file_json = serde_json::to_string(&node).default_res()?;
        let options = FileSaveOptions::builder().version(ChunkVersion(version)).build();
        // ретраи сохранения выглядит крайне сомнительно. Если в контексте запроса держать инфу, что это уже делали, может быть проще
        // хотя тут скорее всего эта инфа будет меняться в каждом ретрае
        let file_meta_size = self.storage.save_chunk(path, FileStream::StringData(Some(file_json)), options).await.default_res()?;

        let mut new = HashSet::new();
        new.insert(StoredFile {
            // from: 0,
            // to: StoredFileRangeEnd::EnfOfFile(file_meta_size),
            version,
            is_keeper: true,
        });

        let (node_type, node_id) = match node {
            FsNode::Folder(folder) => (FsNodeType::Folder, Some(folder.id)),
            FsNode::File(file) => (FsNodeType::File, Some(file.id)),
            FsNode::UserInfo(_) => (FsNodeType::User, None),
            FsNode::RebacPermission(_) => (FsNodeType::Rebac, None)
        };

        if let Err(err) = self.update_state(path, node_id, node_type, &new, &HashSet::new(), prev_seq).await {
            if let Err(delete_err) = self.storage.delete_chunk(path, ChunkVersion(version)).await {
                return Err(format!("main_err: {}; when delete created file: {}", err, delete_err));
            }

            return Err(err);
        }

        let file_path = local_path(path).to_string();
        self.state_updater.send(new_node_to_state_update(node_type, file_path)).await.err().iter()
            .for_each(|v| println!("failed to inform file storage about a new file. Err = {}", v));

        Ok(())
    }

    async fn save_data(&self, mut data: FileStream) -> Result<(DataMetaKey, u64), String> {

        // добавить ретраи (вдруг не уникальное имя)
        let temp_id = random_name(16);
        let temp_path = format!("{}/{}", self.config.get_val(ConfigKey::TempFolder).await, temp_id);

        let channel_capacity = 1000;
        let (tx, storage_rx) = channel(channel_capacity);
        let mut hasher_rx = tx.subscribe();

        // let cloned_temp_path = temp_path.clone();
        let cloned_storage = self.storage.clone();
        let cloned_temp_path = temp_path.clone();
        let file_size_future = actix::spawn(async move {
            let options = FileSaveOptions::builder().version(ChunkVersion(0)).build();
            cloned_storage.save_chunk(&cloned_temp_path, FileStream::ReceiverStream(BroadcastStream::from(storage_rx)), options)
                .await
        });

        let hash_future = tokio::task::spawn_blocking(move || {
            let mut hasher = Sha256::new();
            while let Ok(some) = hasher_rx.blocking_recv() {
                match some {
                    Ok(v) => hasher.update(v),
                    Err(err) => return Err(err)
                }
            }
            return Ok(format!("{:x}", hasher.finalize()))
        });

        while let Some(some) = data.next().await {
            while tx.len() > channel_capacity - 10 {
                tokio::task::yield_now().await;
            }
            tx.send(some).default_res()?;
        }
        drop(tx);

        let file_size = file_size_future.await.default_res()?.default_res()?;

        // while let Some(some) = data.next().await {
        //     tx.send(some.unwrap()).default_res()?;
        // }

        // распаралелить сохранение и получение хеша
        // let mut file_stream = self.storage.get_file(&temp_path, None, Some(ChunkVersion(0)))
        //     .await
        //     .default_res()?
        //     .swap_remove(0);



        let hash = hash_future.await.default_res()?.map_err(|n| format!("status_code = {n}"))?;
        let id_suffix = random_name(5);
        let data_id = DataMetaKey { hash, hash_local_id: id_suffix};
        // добавить datameta
        // из tmp перевести в /data
        // записать обо всем в dht

        let data_meta = DataMeta {
            key: data_id.clone(),
            data_type: DataMetaType::Leaf,
        };
        let data_meta_json = serde_json::to_string(&data_meta).default_res()?;
        let data_meta_path = format!("{}/{}", self.config.get_val(ConfigKey::DataMetaPath).await, data_meta.key.filename());
        let options = FileSaveOptions::builder().version(ChunkVersion(0)).build();
        let data_meta_size = self.storage.save_chunk(&data_meta_path, FileStream::StringData(Some(data_meta_json)), options).await.default_res()?;
        // dht о сохранении сообщили

        let mut new = HashSet::new();
        new.insert(StoredFile {
            // from: 0,
            // to: StoredFileRangeEnd::EnfOfFile(data_meta_size),
            version: 0,
            is_keeper: true,
        });

        self.update_state(&data_meta_path, None, FsNodeType::DataMeta, &new, &HashSet::new(), PrevSeq::Any).await?;
        self.state_updater.send(StateUpdate::NewDataMeta(data_meta_path)).await.err().iter()
            .for_each(|v| println!("failed to inform file storage about a new file. Err = {}", v));

        let mut new = HashSet::new();
        new.insert(StoredFile {
            // from: 0,
            // to: StoredFileRangeEnd::EnfOfFile(file_size),
            version: 0,
            is_keeper: true,
        });

        let local_new_path = format!("{}/{}", self.config.get_val(ConfigKey::DataPath).await, data_id.filename());
        self.storage.move_chunk(&temp_path, &local_new_path).await?;
        self.update_state(&local_new_path, None, FsNodeType::Data, &new, &HashSet::new(), PrevSeq::Any).await?;
        self.state_updater.send(StateUpdate::NewData(local_new_path)).await.err().iter()
            .for_each(|v| println!("failed to inform file storage about a new file. Err = {}", v));

        // считаем sha256 хеш
        // проверяем наличие в dht (ну либо не проверям, а просто добавляем с случайным индексом)
        // меняем tmp файл на что то адекватное (в нужную папку + название файла (hash_id))
        // выходим?

        Ok((data_id, file_size))

    }

    pub async fn delete_user_file(&self, path: &str) -> Result<(), FileDeletingError> {
        if !path.starts_with(&self.config.get_val(ConfigKey::HomePath).await) {
            return Err(FileDeletingError::AccessDenied("no rights to write to the specified folder".to_string()))
        }

        let file_path = PathBuf::from(path);
        if let Some(node_type) = self.check_exist(&file_path).await.default_res()? {
            if node_type != FsNodeType::File {
                return Err(FileDeletingError::Other(format!("{:?} is not a file", &file_path)));
            }
        } else {
            return Err(FileDeletingError::Other(format!("file {:?} does not exist", &file_path)))
        }

        let file = self.delete_file(path).await?;

        retry_fn(|| self.delete_file_from_folder(path))
            .retries(3)
            .custom_backoff(LinearBackoffWithJitter::new(Duration::from_secs(1), 2, 30))
            .await
            .map_err(|err| format!("couldn't save file to folder. child err: {err}"))?;

        Ok(())
    }

    async fn delete_file(&self, path: &str) -> Result<(), String>{
        todo!("delete file")
    }

    async fn delete_file_from_folder(&self, path: &str) -> Result<(), String> {
        todo!("delete file from folder")
    }

    pub async fn put_user_file(&self, path: &str, data: FileStream, range_o: Option<Range>) -> Result<FileMeta, FileSavingError> {
        if !path.starts_with(&self.config.get_val(ConfigKey::HomePath).await) {
            return Err(FileSavingError::AccessDenied("no rights to write to the specified folder".to_string()))
        }

        let mut root_folder = PathBuf::from(path);
        root_folder.pop();
        if let Some(node_type) = self.check_exist(&root_folder).await.default_res()? {
            if node_type != FsNodeType::Folder {
                return Err(FileSavingError::Other(format!("{:?} is not a folder", &root_folder)));
            }
        } else {
            return Err(FileSavingError::Other(format!("folder {:?} folder does not exist", &root_folder)))
        }

        let file = self.put_file(path, data, range_o).await?;

        retry_fn(|| self.put_file_into_folder(Path::new(path), &root_folder))
            .retries(3)
            .custom_backoff(LinearBackoffWithJitter::new(Duration::from_secs(1), 2, 30))
            .await
            .map_err(|err| format!("couldn't save file to folder. child err: {err}"))?;

        Ok(file)
    }

    pub async fn put_file(&self, path: &str, data: FileStream, range_o: Option<Range>) -> Result<FileMeta, FileSavingError> {
        // if self.check_exist(Path::new(path)).await.default_res()?.is_some() {
        //     return Err(FileSavingError::AlreadyExist);
        // }

        // заборы про пути строим тут

        range_o.iter().for_each(|_| todo!("put with range"));
        let mut file_range = parse_range(range_o)?;
        if file_range.len() > 1 {
            return Err(FileSavingError::InvalidRange);
        }
        let file_range = file_range.swap_remove(0)?;

        let (locked_version, prev_file_meta) = retry_fn(|| self.lock_next_file_version(path, FsNodeType::File))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(3), 30))
            .await?;

        /*
        if let Some(prev_file_keepers) = prev_file_keepers {
            let mut max_version = 0;
            let mut keeper_of_last = None;
            for keeper in &prev_file_keepers.keepers {
                for file in &keeper.ranges {
                    if file.version >= max_version && file.is_keeper {
                        max_version = file.version;
                        keeper_of_last = Some(keeper.id);
                    }
                }
            }
            if let Some(keeper_of_last) = keeper_of_last {
                let file_meta = self.client.get_file_meta(path, keeper_of_last).await?;
            }
            // не очень понятно зачем это вообще нужно
        }

         */

        let file_id_o = prev_file_meta
            .map(|v| v.id);

        let file_id = if let Some(some_id) = file_id_o {
            some_id
        } else {
            self.lock_generated_id(path).await?
        };

        // нужно проверить что не загружаем то что уже есть
        let is_success = (|| async {
            let (data_id, data_size) = self.save_data(data).await?;
            let file_meta = FsNode::File(File {
                path: path.to_string(),
                id: file_id,
                data: vec![((0, data_size), data_id.clone())]
            });
            self.save_node(file_meta.get_path().unwrap(), &file_meta, locked_version, PrevSeq::Any).await
        })().await;


        // создал мету об data, сохранил мету
        // создал мету о файле, добавил туда ссылку на data, сохранил мету
        // в мету о корневой папке добвил мету о созданном файлике

        let mut lock = HashSet::new();
        lock.insert(StoredFile {
            // from: 0,
            // to: StoredFileRangeEnd::EndOfRange(0),
            version: locked_version,
            is_keeper: false
        });
        self.update_state(path, Some(file_id), FsNodeType::File, &HashSet::with_capacity(0), &lock, PrevSeq::Any).await.err().iter()
            .for_each(|v| println!("information about the new file could not be saved to dht. Err = {}", v));

        is_success?; // waiting for the release of the lock



        Ok(FileMeta::builder().name(path.to_string()).node_type(NodeType::File).build())
    }

    async fn put_file_into_folder(&self, filepath: &Path, folder: &Path) -> Result<(), String> {
        // добавить ретраи
        let folder_path_str = folder.to_str().unwrap();
        let folder_json = self.dht_map.get(folder_path_str).await.default_res()?;
        let folder_json = folder_json.ok_or(format!("{:?} does not exist", folder))?;

        let mut folder_keepers = serde_json::from_str::<GlobalFileInfo>(&folder_json.0).default_res()?;

        if folder_keepers.file_type != FsNodeType::Folder {
            return Err(format!("{:?} is not a folder", folder));
        }

        let keeper_o = folder_keepers.keepers.iter()
            .max_by_key(|v| v.data.iter().max_by_key(|v| v.version).unwrap().version);
        let keeper = keeper_o.ok_or("No one keeps the file with the folder's metadata".to_string())?;

        let folder_stream = self.client.get_file_content(folder_path_str, keeper.id).await?;
        let folder_str = read_stream_to_string(folder_stream.map_err(|v| v.to_string())).await?;

        let mut folder = serde_json::from_str::<FsNode>(&folder_str).default_res()?;

        if let FsNode::Folder(folder) = &mut folder {
            let filepath_str = filepath.to_str().unwrap();
            if folder.refs.iter().any(|v| v.as_str() == filepath_str) {
                return Ok(())
            }
            folder.refs.push(filepath_str.to_string());
        } else {
            return Err(format!("{:?} is not a folder", folder));
        }

        let node_version = keeper.data.iter().max_by_key(|v| v.version).unwrap().version + 1;
        self.save_node(folder.get_path().unwrap(), &folder, node_version, PrevSeq::Seq(Some(folder_json.1))).await

        /*
                let folder_str = serde_json::to_string(&folder).default_res()?;
        let size = self.storage.save(
            folder_path_str,
            (0, EndOfFileRange::LastByte),
            FileStream::StringData(Some(folder_str)),
            FileSaveOptions::builder().version(ChunkVersion((folder_json.1 + 1) as u64)).build()
        ).await.default_res()?;


        let mut new_folder = HashSet::with_capacity(1);
        new_folder.insert(StoredFileRange {
            from: 0,
            to: StoredFileRangeEnd::EnfOfFile(size),
            version: 0,
            is_keeper: false,
        });
        self.update_state(
            folder_path_str,
            FsNodeType::Folder,
            &new_folder,
            &HashSet::with_capacity(0),
            PrevSeq::Seq(Some(folder_json.1))
        ).await

         */
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

fn random_name(len: usize) -> String {
    RANDOM.take()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn local_path(path: &str) -> &str {
    path.trim_start_matches("/")
}

fn new_node_to_state_update(fs_node_type: FsNodeType, path: String) -> StateUpdate {
    match fs_node_type {
        FsNodeType::Folder => StateUpdate::NewFolder(path),
        FsNodeType::File => StateUpdate::NewFile(path),
        FsNodeType::DataMeta => StateUpdate::NewDataMeta(path),
        FsNodeType::Data => StateUpdate::NewData(path),
        FsNodeType::Rebac => StateUpdate::NewRebac(path),
        FsNodeType::User => StateUpdate::NewUser(path)
    }
}

fn fs_node_type_to_external(fs_node_type: FsNodeType) -> NodeType {
    match fs_node_type {
        FsNodeType::Folder => NodeType::Folder,
        FsNodeType::File => NodeType::File,
        FsNodeType::DataMeta => NodeType::File,
        FsNodeType::Data => NodeType::File,
        FsNodeType::Rebac => NodeType::File,
        FsNodeType::User => NodeType::File
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_http_range() {
        assert_eq!(0, 0);
    }

    #[test]
    fn test_parse_dht_value() {
        assert_eq!(0, 0);
    }
}