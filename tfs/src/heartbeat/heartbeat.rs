use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Index;
use std::panic;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use actix_web::http::header::{ByteRangeSpec, Range};
use dashmap::DashMap;
use futures::future::join_all;
use futures::{StreamExt, TryStreamExt};
use itertools::Itertools;
use log::{debug, error, info};
use rand::Rng;
use rand::rngs::ThreadRng;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use typed_builder::TypedBuilder;
use crate::common::common::read_stream_to_string;
use crate::common::default_error::DefaultError;
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::common::models::{ObjType, PermissionType, UserInfo};
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::heartbeat::errors::UpdateDhtError;
use crate::heartbeat::models::{RangeEvent, RangeEventType, ReplicateFileError, ReplicateFileStatus};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::{DhtNodeId, PrevSeq};
use crate::services::file_storage::errors::FileReadingError;
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::file_storage::model::{ChunkVersion, FileSaveOptions, NodeType};
use crate::services::internal_communication::InternalCommunication;
use crate::services::permission_service::permission_service::PermissionService;
use crate::services::virtual_fs::models::{FileKeeper, Folder, FsNode, FsNodeType, GlobalFileInfo, StoredFile, StoredFileRangeEnd};
use crate::services::virtual_fs::virtual_fs::VirtualFS;
// static dht: DistributedMap = 0;

#[derive(TypedBuilder)]
pub struct Heartbeat {
    fs: Arc<VirtualFS>,
    config: Arc<GlobalConfig>,
    storage: Arc<dyn FileStorage + Send + Sync>,
    permission_service: Arc<PermissionService>,

    #[builder(default=Arc::new(DashMap::new()))]
    state: Arc<DashMap<String, FsNodeType>>,
    client: InternalCommunication,
    cancellation_token: CancellationToken
}

thread_local! {
    // pub static CLIENT: RefCell<InternalCommunication> = RefCell::new(InternalCommunication::default());
    pub static RANDOM: RefCell<ThreadRng> = RefCell::new(rand::thread_rng());
}
impl Heartbeat {
    pub async fn init(&self, mut update_receiver: Receiver<StateUpdate>) {

        let map_ref = self.state.clone();
        let updater = tokio::spawn(async move {
            while let Some(update) = update_receiver.recv().await {
                match update {
                    StateUpdate::NewFile(path) => map_ref.insert(path, FsNodeType::File),
                    StateUpdate::NewFolder(path) => map_ref.insert(path, FsNodeType::Folder),
                    StateUpdate::NewDataMeta(path) => map_ref.insert(path, FsNodeType::DataMeta),
                    StateUpdate::NewData(path) => map_ref.insert(path, FsNodeType::Data),
                    StateUpdate::NewRebac(path) => map_ref.insert(path, FsNodeType::Rebac),
                    StateUpdate::NewUser(path) => map_ref.insert(path, FsNodeType::User)
                };
            }
        });

        let mut init_db_res = self.init_db().await;
        init_db_res.as_ref().err().iter().for_each(|v| error!("err when init db: {}", v));

        let mut initialized = init_db_res.is_ok();
        while !initialized {
            tokio::time::sleep(Duration::from_secs(5)).await;

            init_db_res = self.init_db().await;
            init_db_res.as_ref().err().iter().for_each(|v| error!("err when init db: {}", v));

            initialized = init_db_res.is_ok();
        }

        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            // panic::catch_unwind(|| async {

                if let Err(err) = self.replicate_files().await {
                    error!("replication err {}", err);
                }

            // });
            if self.cancellation_token.is_cancelled() {
                updater.abort();
                break;
            }
        }
        tokio::join!(updater);
    }


    async fn init_db(&self) -> Result<(), String>{
        let home_path = self.config.get_val(ConfigKey::HomePath).await;
        let data_path = self.config.get_val(ConfigKey::DataPath).await;
        let data_meta_path = self.config.get_val(ConfigKey::DataMetaPath).await;
        let temp_path = self.config.get_val(ConfigKey::TempFolder).await;
        let rbac_path = self.config.get_val(ConfigKey::RebacPath).await;
        let users_path = self.config.get_val(ConfigKey::UsersPath).await;

        let distributed_folders = [
            (data_path.clone(), Some(FsNodeType::Data)),
            (data_meta_path.clone(), Some(FsNodeType::DataMeta)),
            (home_path.clone(), None),
            (rbac_path.clone(), Some(FsNodeType::Rebac)),
            (users_path.clone(), None)
        ];

        let all_folders = [
            data_path.clone(),
            data_meta_path.clone(),
            home_path.clone(),
            temp_path.clone(),
            rbac_path.clone(),
            users_path.clone()
        ];

        join_all(
            all_folders.iter().map(|folder| self.storage.create_folder(&folder))
        ).await;

        self.init_home_folder(&home_path).await.err().iter().for_each(|v| error!("{v}"));
        self.init_service_permissions().await.err().iter().for_each(|v| error!("{v}"));
        self.init_user_folder(&users_path).await.err().iter().for_each(|v| error!("{v}"));

        let mut res = Vec::new();
        res.push(self.scan_file(&home_path, None).await.map_err(|err| (home_path, err)));
        res.extend(join_all(
            distributed_folders.into_iter()
                .map(|path| async move {
                    self.scan_folder(&path.0, path.1).await.map_err(|err| (path.0, err))
                })
        ).await.into_iter());

        let errors: Vec<String> = res.into_iter()
            .filter(Result::is_err)
            .map(|v| v.err().unwrap())
            .map(|v| format!("path: {}, err: {}", v.0, v.1))
            .collect();

        if errors.len() != 0 {
            let error = errors.join("; ");
            error!("err when heartbeat::init_db; {}", error);
            return Err(error);
        }

        Ok(())
    }

    async fn init_home_folder(&self, home_path: &str) -> Result<(), String> {
        let empty_home = FsNode::Folder(
            Folder::builder()
                .path(home_path.to_string())
                .id(u64::from_str(&self.config.get_val(ConfigKey::HomeFolderId).await).unwrap())
                .build()
        );
        self.storage.save_chunk(
            home_path,
            FileStream::StringData(Some(serde_json::to_string(&empty_home).default_res()?)),
            FileSaveOptions::builder().version(ChunkVersion(0)).build()
        ).await.err().iter().for_each(|err| error!("when creating the home folder: {}", err));

        Ok(())
    }

    async fn init_user_folder(&self, user_folder_path: &str) -> Result<(), String> {
        let service_uid = u64::from_str(&self.config.get_val(ConfigKey::SelfUid).await).unwrap();
        let service_user_file = FsNode::UserInfo(
            UserInfo {
                uid: service_uid,
                login: "service".to_string(),
                hashed_password: "".to_string(),
            }
        );
        self.storage.save_chunk(
            &format!("{}/{}", user_folder_path, service_uid),
            FileStream::StringData(Some(serde_json::to_string(&service_user_file).default_res()?)),
            FileSaveOptions::builder().version(ChunkVersion(0)).build()
        ).await.err().iter().for_each(|err| error!("when creating the service user file: {}", err));

        let anonymous_uid = u64::from_str(&self.config.get_val(ConfigKey::AnonymousUid).await).unwrap();
        let anonymous_user_file = FsNode::UserInfo(
            UserInfo {
                uid: anonymous_uid,
                login: "anonymous".to_string(),
                hashed_password: format!("{:x}", Sha256::digest("anonymous")),
            }
        );
        self.storage.save_chunk(
            &format!("{}/{}", user_folder_path, anonymous_uid),
            FileStream::StringData(Some(serde_json::to_string(&anonymous_user_file).default_res()?)),
            FileSaveOptions::builder().version(ChunkVersion(0)).build()
        ).await.err().iter().for_each(|err| error!("when creating the anonymous user file: {}", err));
        Ok(())
    }

    async fn init_service_permissions(&self) -> Result<(), String> {
        let service_uid_string = self.config.get_val(ConfigKey::SelfUid).await;
        let service_uid = u64::from_str(&service_uid_string).unwrap();

        let folder_id_keys = [ConfigKey::DataFolderId, ConfigKey::DataMetaFolderId, ConfigKey::HomeFolderId,
            ConfigKey::RebacFolderId, ConfigKey::UsersFolderId];

        for folder_id_key in folder_id_keys {
            let folder_id = u64::from_str(&self.config.get_val(folder_id_key).await).unwrap();

            self.permission_service.put_permission_without_rights_check(service_uid, ObjType::Folder,
                                                                        folder_id, PermissionType::All,
                                                                        Some(0)
            ).await?
        }

        Ok(())
    }

    async fn get_file_type(&self, path: &str) -> Result<FsNodeType, String> {
        let mut file = self.storage.get_file(&path, None, None)
            .await.default_res()?
            .swap_remove(0);

        let json = read_stream_to_string(file.1.get_stream()).await?;
        // let mut json_vec: Vec<u8> = Vec::with_capacity(50);
        // while let Some(some) = file.1.next().await {
        //     if let Ok(v) = some {
        //         json_vec.extend(v.as_ref().clone())
        //     } else {
        //         return Err(format!("error when read file. err = {}", some.err().unwrap()))
        //     }
        // }
        // let json = String::from_utf8(json_vec)
        //     .default_res()?;
        let node: FsNode = serde_json::from_str(&json).default_res()?;

        match node {
            FsNode::Folder(_) => Ok(FsNodeType::Folder),
            FsNode::File(_) => Ok(FsNodeType::File),
            FsNode::UserInfo(_) => Ok(FsNodeType::User),
            FsNode::RebacPermission(_) => Ok(FsNodeType::Rebac)
        }
    }

    async fn scan_file(&self, filepath: &str, override_file_type: Option<FsNodeType>) -> Result<(), String> {
        let file_type = if let None = override_file_type {
            self.get_file_type(filepath).await?
        } else {
            override_file_type.unwrap()
        };

        self.state.insert(filepath.to_string(), file_type);
        Ok(())
    }

    async fn scan_folder(&self, root: &str, override_file_type: Option<FsNodeType>) -> Result<(), String> {
        let folder = self.storage.get_folder_content(root).await.default_res()?;
        for file in folder.files {
            match file.node_type {
                NodeType::File => {
                    self.scan_file(&format!("{root}/{}", &file.name), override_file_type).await?;
                }
                NodeType::Folder => {
                    let new_root = if root == "/" { format!("/{}", file.name) } else { format!("{}/{}", root, file.name) };
                    Box::pin(self.scan_folder(&new_root, override_file_type)).await?;
                }
            }
        }
        Ok(())

    }

    async fn replicate_files(&self) -> Result<(), String> {
        self.fs.init().await?;

        // если в dht версия ниже, то это невозможно, бекапим запись. Если отсутствует или больше, то допустим что ок
        debug!("{:?}", self.state);
        let keepers_result = self.fs.get_all_keepers().await.map(|mut keepers| {
            if let Some(index) = keepers.iter().position(|v| v == self.fs.get_id()) {
                keepers.swap_remove(index);
            }
            return keepers;
        });
        debug!("keepers: {:?}", keepers_result);
        info!("replication has started");
        // тут замечаем слишком старые версии и перестаем их обновлять

        let replication_factor = u32::from_str(&self.config.get_val(ConfigKey::ReplicationFactor).await).unwrap();
        let network_keepers = keepers_result?;
        if network_keepers.is_empty() {
            return Err("Too few keepers".to_string());
        }
        for path in self.state.iter() {
            self.replicate_file(path.key(), *path.value(), &network_keepers, replication_factor).await
                .err().iter().for_each(|v| error!("{v}"));
            match path.value() {
                FsNodeType::Folder => self.reinit_file_mapping(path.key()).await
                    .err().iter().for_each(|v| error!("{v}")),
                FsNodeType::File => self.reinit_file_mapping(path.key()).await
                    .err().iter().for_each(|v| error!("{v}")),
                FsNodeType::DataMeta => {}
                FsNodeType::Data => {}
                FsNodeType::Rebac => self.reinit_rebac_rule(path.key()).await
                    .err().iter().for_each(|v| error!("{v}")),
                FsNodeType::User => self.reinit_user_uid_mapping(path.key()).await
                    .err().iter().for_each(|v| error!("{v}"))
            }
        }
        info!("replication is complete");


        Ok(())
    }

    async fn reinit_rebac_rule(&self, path: &str) -> Result<(), String> {
        let (range, stream) = self.get_all_file(path).await?;
        let rebac_rule_string = read_stream_to_string(stream.get_stream()).await?;
        let rebac_rule = serde_json::from_str::<FsNode>(&rebac_rule_string).default_res()?;

        if let FsNode::RebacPermission(permission) = rebac_rule {
            let obj_type = permission.object.obj_type;
            let obj_id = permission.object.obj_id;

            let stored_permissions = self.permission_service.get_permissions(permission.subject, obj_type, obj_id)
                .await?;
            if !stored_permissions.contains(&permission.permission_type) && !permission.deleted {
                self.permission_service.put_permission_without_rights_check(
                    permission.subject, obj_type, obj_id, permission.permission_type, Some(permission.version)
                ).await?;
            }
            if permission.deleted {
                self.permission_service.delete_permission_without_rights_check(
                    permission.subject, obj_type, obj_id, permission.permission_type, Some(permission.version)
                ).await?;
            }

            Ok(())
        } else {
            Err("invalid file type when reinit_rebac_rule".to_string())
        }

    }

    async fn reinit_user_uid_mapping(&self, path: &str) -> Result<(), String> {
        let (range, stream) = self.get_all_file(path).await?;
        let node_string = read_stream_to_string(stream.get_stream()).await?;
        let node = serde_json::from_str::<FsNode>(&node_string).default_res()?;

        let mapping = match node {
            FsNode::Folder(_) => None,
            FsNode::File(_) => None,
            FsNode::UserInfo(user) => Some((user.uid, user.login)),
            FsNode::RebacPermission(_) => None
        };

        if let Some(some_mapping) = mapping {
            let dht_uid = self.fs.uid_by_login(&some_mapping.1).await?;
            if let Some(some_uid) = dht_uid {
                if some_mapping.0 == some_uid {
                    return Ok(())
                } else {
                    error!("expected user login {} mapping {}, but {}", some_mapping.1, some_mapping.0, some_uid);
                    error!("научиться обновлять, а не только писать новые записи");
                }
            }

            self.fs.lock_user_uid_mapping(some_mapping.0, &some_mapping.1).await?
        }

        Ok(())
    }

    async fn reinit_file_mapping(&self, path: &str) -> Result<(), String> {
        let (range, stream) = self.get_all_file(path).await?;
        let node_string = read_stream_to_string(stream.get_stream()).await?;
        let node = serde_json::from_str::<FsNode>(&node_string).default_res()?;

        let mapping = match node {
            FsNode::Folder(folder) => {
                Some((folder.id, folder.path))
            }
            FsNode::File(file) => {
                Some((file.id, file.path))
            }
            FsNode::UserInfo(_) => None,
            FsNode::RebacPermission(_) => None
        };

        if let Some(some_mapping) = mapping {
            let dht_filename = self.fs.filename_by_id(some_mapping.0).await?;
            if let Some(some_dht_filename) = dht_filename {
                if some_mapping.1 == some_dht_filename {
                    return Ok(())
                }
            }

            self.fs.lock_id(some_mapping.0, &some_mapping.1).await?
        }

        Ok(())
    }

    async fn replicate_file(&self, path: &str, fs_node_type: FsNodeType, network_keepers: &[DhtNodeId],
                            replication_factor: u32) -> Result<ReplicateFileStatus, ReplicateFileError> {

        let mut path_to_delete = Vec::new();
        let mut versions_to_replicate = Vec::new();
        let number_of_stored_versions = u64::from_str(&self.config.get_val(ConfigKey::NumberOfStoredVersions).await).default_res()?;
        let max_version_delta = u64::max(1, number_of_stored_versions) - 1u64;
        let self_id = self.fs.get_id();

        let mut self_updated = false;
        let self_stored = self.storage.get_file_meta(path).await.default_res()?; // тут нужно чет делать, просто откидывать не вариант
        let meta = self.fs.get_node_meta(path).await;
        if meta.is_ok() && !meta.as_ref().unwrap().keepers.is_empty() {
            let meta = meta.unwrap();

            let global_max = meta.keepers.iter()
                .flat_map(|v| &v.data)
                .filter(|v| v.is_keeper)
                .max_by_key(|v| v.version)
                .ok_or(format!("empty dht node: {:?}", meta))?
                .version;

            let mut local_only_versions: HashSet<u64> = meta.keepers.iter()
                .filter(|v| v.id == *self_id)
                .flat_map(|v| &v.data)
                .filter(|v| v.is_keeper)
                .map(|v| v.version)
                .collect();

            meta.keepers.iter()
                .filter(|v| v.id != *self_id)
                .flat_map(|v| &v.data)
                .for_each(|v| {
                    local_only_versions.remove(&v.version);
                });


            let min_version = u64::checked_sub(global_max, max_version_delta).unwrap_or(0);

            let self_index_o = meta.keepers.iter().position(|v| v.id == *self_id);
            if let Some(self_index) = self_index_o {
                let self_keeper = &meta.keepers[self_index];
                if let Err(r) = self.update_dht_state(path, fs_node_type, &self_keeper.data).await {
                    match r {
                        UpdateDhtError::FileNotFound(_) => {
                            path_to_delete.push(path)
                        }
                        UpdateDhtError::InternalError(_) => {}
                    }
                    return Err(ReplicateFileError::InternalError("error when update self info in dht".to_string()));
                }
                self_updated = true;

                let mut hasher = DefaultHasher::new();
                meta.filename.hash(&mut hasher);
                let filename_hash = hasher.finish();


                if filename_hash % meta.keepers.len() as u64 != self_index as u64
                    && local_only_versions.is_empty() { // добавить обработку отключения мастер ноды (можно просто рандом добавить)

                    return Ok(ReplicateFileStatus::NotMaster)
                }
            }

            let mut stored_versions: HashMap<u64, u32> = self_stored.iter()
                .filter(|v| v.is_keeper && v.version >= min_version)
                .map(|v| (v.version, 0)).collect();

            meta.keepers.iter()
                .filter(|v| v.id != *self_id)
                .flat_map(|v| &v.data)
                .filter(|v| v.is_keeper && v.version >= min_version)
                .for_each(|v|
                    if stored_versions.contains_key(&v.version) {
                        *stored_versions.get_mut(&v.version).unwrap() += 1;
                    }
                );
            // отдельно обработать случай, когда инфу о себе ещё не сохранили


            stored_versions.iter()
                .filter(|(&version, &copies)| copies < replication_factor - 1)
                .for_each(|(version, copies)| versions_to_replicate.push(*version));


            for keeper in &meta.keepers {
                if keeper.id != *self.fs.get_id() {
                    debug!("keeper file {} info: {:?}", path, keeper);

                    // let other_keeper_dht_info: HashSet<StoredFile> = meta.keepers.iter()
                    //     .filter(|v| v.id == keeper.id)
                    //     .flat_map(|v| v.ranges)
                    //     .map(|v| v.is_keeper == true)
                    //     .collect();

                    // let other_keeper_stored = CLIENT.take().get_stored_versions_of_file(path.key(), keeper.id).await?;
                    // let other_stored_set = HashSet::from(other_keeper_stored);
                    // нужно удалять только если узел долго не отвечает, иначе он и сам все нужное может удалить
                }
            }
        }
        if !self_updated {
            if let Err(err) = self.update_dht_state(path, fs_node_type, &vec![]).await {
                error!("heartbeat::replication_files error when update_dht_state. {}", err.to_string());
                return Err(ReplicateFileError::InternalError(format!("error when update_dht_state {err}")))
            }
        }


        // все ещё нужно реализовать проверку что реплицируешь именно ты, с каким то таймаутом ожидания,
        // после которого реплицирует кто то другой

        // нужно подумать как тут не отослать узлу, у которого уже есть этот чанк
        // если полная репликация на все шарды, то нужно как-то рандом к этом приготовить, может больше ретраев сделать,
        // и проверять среди уже известной инфы, а не ходить каждый раз по сети
        // let segment = local_chunks[i];

        let mut new_keeper = None;
        let mut checked = HashSet::new();
        let mut not_replicated = Vec::new();
        for bad_version in versions_to_replicate {
            let version_key = StoredFile::builder()
                .version(bad_version)
                .is_keeper(true)
                .build();

            for _ in 0..(replication_factor * 3) {
                let keeper_n = RANDOM.take().gen_range(0..network_keepers.len());
                let keeper = network_keepers[keeper_n];

                if checked.contains(&keeper) {
                    continue;
                }
                let already_stored_o = self.client.get_stored_versions_of_file(path, keeper).await;
                checked.insert(keeper);
                if let Ok(already_stored) = already_stored_o {
                    if already_stored.contains(&version_key) {
                        debug!("already stored file {}: {:?}", path, already_stored);
                    } else {
                        debug!("already stored {} : not all", path);
                        new_keeper = Some(keeper);
                        break;
                    }
                } else {
                    debug!("already stored {} : None", path);
                    new_keeper = Some(keeper);
                    break;
                }
            }
            let new_keeper = if let Some(keeper) = new_keeper {
                keeper
            } else {
                not_replicated.push(bad_version);
                continue;
            };


            let file_version = ChunkVersion(bad_version);
            let mut file = self.storage.get_file(path, None, Some(file_version))
                .await.default_res()?.swap_remove(0);

            let range =  ByteRangeSpec::From(0);
            debug!("send file {} with range {} {:?} v{}", path, 0, file.0.1, bad_version);
            if let Err(e) = self.client.send_file(path, new_keeper,
                                                    file.1,
                                                    Range::Bytes(vec![range]), fs_node_type, bad_version).await {
                error!("error while send file {} to {:?}. err: {}", path, new_keeper, e);
            }
        }
        if !not_replicated.is_empty() {
            Err(ReplicateFileError::NotFoundNewKeeper(not_replicated))
        } else {
            Ok(ReplicateFileStatus::Success)
        }

    }

    async fn update_dht_state(&self, filename: &str, file_type: FsNodeType, dht_state: &[StoredFile]) -> Result<(), UpdateDhtError> {
        let mut to_add_to_dht = Vec::new();
        let stored_local = match self.storage.get_file_meta(filename).await {
            Ok(v) => v,
            Err(err) => {
                return Err(match err {
                    FileReadingError::NotExist => UpdateDhtError::FileNotFound("file does not exist".to_string()),
                    FileReadingError::BadRequest => UpdateDhtError::InternalError("bad request".to_string()),
                    FileReadingError::ChunkIsNotExist(v) => UpdateDhtError::FileNotFound("file does not exist".to_string()),
                    FileReadingError::Retryable(v) => UpdateDhtError::InternalError(v),
                    FileReadingError::InternalError(v) => UpdateDhtError::InternalError(v),
                    FileReadingError::AccessDenied => UpdateDhtError::InternalError("access denied".to_string())
                });
            }
        };
        let mut dht_chunks = dht_state.iter()
            .map(|v| *v)
            .filter(|v| v.is_keeper)
            .collect::<HashSet<StoredFile>>();

        let max_version = stored_local.iter()
            .max_by_key(|v| v.version)
            .map(|v| v.version)
            .unwrap_or(0);
        let max_version_delta = u64::checked_sub(
            u64::from_str(&self.config.get_val(ConfigKey::NumberOfStoredVersions).await).default_res()?,
            1
        ).unwrap_or(0);
        let min_allowed_version = u64::checked_sub(max_version, max_version_delta)
            .unwrap_or(0);

        for local_range in stored_local {
            if local_range.version < min_allowed_version {
                continue
            }
            if !dht_chunks.contains(&local_range) {
                to_add_to_dht.push(local_range)
            } else {
                dht_chunks.remove(&local_range);
            }
        }

        let node_id = match file_type {
            FsNodeType::Folder | FsNodeType::File  => {
                let (range, stream) = self.get_all_file(filename).await
                    .map_err(|err| UpdateDhtError::InternalError(err))?;

                // let stream = file.swap_remove(0).1;
                let file = read_stream_to_string(stream.get_stream()).await?;
                let node = serde_json::from_str::<FsNode>(&file)
                    .map_err(|err| format!("when parse file {}: {}", filename, err))?;
                match node {
                    FsNode::Folder(folder) => Some(folder.id),
                    FsNode::File(file) => Some(file.id),
                    _ => None
                }
            }
            FsNodeType::DataMeta => None,
            FsNodeType::Data => None,
            FsNodeType::Rebac => None,
            FsNodeType::User => None
        };

        let to_delete: HashSet<StoredFile> = dht_chunks.iter().map(|v| *v).collect();

        debug!("state update: file: {} delete {:?}, add: {:?}", filename, to_delete, to_add_to_dht);
        if to_delete.len() != 0 || to_add_to_dht.len() != 0 {
            let res = self.fs.update_state(filename,
                                           node_id,
                                           file_type,
                                           &to_add_to_dht.iter().map(|v| *v).collect(),
                                           &to_delete,
                                           PrevSeq::Any
            ).await;
            if let Err(err) = res {
                error!("err: {}", err.to_string());
            }
        }

        Ok(())
    }

    async fn get_all_file(&self, path: &str) -> Result<(&FileRange, FileStream), String> {
        let mut file = self.storage.get_file(path, None, None)
            .await
            .default_res()?;
        if file.len() != 1 {
            return Err(
                format!("storage.get_file returned unexpected num of ranges: len = {}", file.len())
            )
        }
        Ok(file.swap_remove(0))
    }
}

fn get_under_replicated_segments(events: &mut [RangeEvent], replication_factor: u32) -> HashSet<usize> {
    events.sort_by_key(|k| k.event.get_index());

    let mut num_of_copies = HashMap::new();;
    let mut bad_segments = HashSet::new();
    let mut stored = HashMap::new();
    for event in events {
        match event.event {
            RangeEventType::Start(v) => *num_of_copies.entry(event.chunk_version).or_insert(0) += 1,
            RangeEventType::End(v) => {
                *num_of_copies.get_mut(&event.chunk_version).unwrap() -= 1;
                if *num_of_copies.get(&event.chunk_version).unwrap_or(&0) < replication_factor {
                    for segment in stored.get(&event.chunk_version).unwrap_or(&HashSet::with_capacity(0)) {
                        bad_segments.insert(*segment);
                    }
                    stored.get_mut(&event.chunk_version).map(HashSet::clear);
                }
            },
            RangeEventType::StoredStart(v, index) => {
                stored.entry(event.chunk_version).or_insert(HashSet::new()).insert(index);
                if *num_of_copies.get(&event.chunk_version).unwrap_or(&0) < replication_factor {
                    bad_segments.insert(index);
                }
            }
            RangeEventType::StoredEnd(v, index) => {
                stored.get_mut(&event.chunk_version).map(|map| map.remove(&index));
                if *num_of_copies.get(&event.chunk_version).unwrap_or(&0) < replication_factor {
                    bad_segments.insert(index);
                }
            }
        }
    }
    bad_segments
}