use super::*;
use crate::common::file_range::FileRange;
use crate::config::global_config::GlobalConfig;
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::DhtNodeId;
use crate::services::dtls_revocation_registry::open_loopback_map;
use crate::services::file_storage::errors::{CreateFolderError, FolderReadingError};
use crate::services::file_storage::local_storage::local_storage::LocalStorage;
use crate::services::file_storage::model::FolderMeta;
use crate::services::internal_communication::InternalCommunication;
use crate::services::secured_dht::RevocationAdmin;
use async_trait::async_trait;
use std::sync::Arc;

mod identity;
pub(super) use identity::test_root;

pub(super) struct Fixture {
    pub vfs: Arc<VirtualFS>,
    pub map: Arc<DHTMap>,
    pub config: Arc<GlobalConfig>,
    pub local: Arc<LocalStorage>,
    pub notifications: tokio::sync::mpsc::Receiver<StateUpdate>,
    directory: std::path::PathBuf,
    _peer: RevocationAdmin,
}

impl Fixture {
    pub async fn new(fail_readback: bool) -> Self {
        let (map, peer, root) = open_loopback_map().await;
        let config = Arc::new(GlobalConfig::new(map.clone(), root.to_vec()).await);
        let directory = std::env::temp_dir().join(format!(
            "tfs-user-roles-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let local = Arc::new(LocalStorage {
            base_path: directory.display().to_string(),
        });
        let id = DhtNodeId {
            ip: "127.0.0.1".parse().unwrap(),
            port: 12345,
        };
        let storage: Arc<dyn FileStorage + Send + Sync> = Arc::new(ReadbackStorage {
            local: local.clone(),
            fail_readback,
        });
        let (sender, notifications) = tokio::sync::mpsc::channel(4);
        let vfs = Arc::new(
            VirtualFS::builder()
                .dht_map(map.clone())
                .storage(storage)
                .id(id)
                .config(config.clone())
                .state_updater(sender)
                .client(InternalCommunication::new(config.clone()))
                .build(),
        );
        let json = serde_json::to_string(&FsNode::UserInfo(UserInfo {
            uid: 42,
            login: "alice".into(),
            hashed_password: "original-hash".into(),
            roles: vec![Role::RawDhtAccess],
        }))
        .unwrap();
        local
            .save_chunk(
                "users/42",
                FileStream::StringData(Some(json)),
                FileSaveOptions::builder().version(ChunkVersion(0)).build(),
            )
            .await
            .unwrap();
        let metadata = GlobalFileInfo {
            filename: "users/42".into(),
            file_type: FsNodeType::User,
            id: 0,
            keepers: vec![FileKeeper {
                id,
                data: vec![StoredFile {
                    version: 0,
                    is_keeper: true,
                }],
            }],
        };
        map.put_with_seq("users/42", &metadata, PrevSeq::Seq(None))
            .await
            .unwrap();
        Self {
            vfs,
            map,
            config,
            local,
            notifications,
            directory,
            _peer: peer,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

struct ReadbackStorage {
    local: Arc<LocalStorage>,
    fail_readback: bool,
}

#[async_trait(?Send)]
impl FileStorage for ReadbackStorage {
    async fn save_chunk(
        &self,
        path: &str,
        data: FileStream,
        options: FileSaveOptions,
    ) -> Result<u64, FileSavingError> {
        self.local.save_chunk(path, data, options).await
    }
    async fn save(
        &self,
        path: &str,
        range: FileRange,
        data: FileStream,
        options: FileSaveOptions,
    ) -> Result<u64, FileSavingError> {
        self.local.save(path, range, data, options).await
    }
    async fn get_file<'a>(
        &self,
        path: &str,
        ranges: Option<&'a [FileRange]>,
        version: Option<ChunkVersion>,
    ) -> Result<Vec<(&'a FileRange, FileStream)>, FileReadingError> {
        if self.fail_readback && version == Some(ChunkVersion(1)) {
            return Err(FileReadingError::Retryable(
                "injected publication readback failure".into(),
            ));
        }
        self.local.get_file(path, ranges, version).await
    }
    async fn get_file_meta(&self, path: &str) -> Result<Vec<StoredFile>, FileReadingError> {
        self.local.get_file_meta(path).await
    }
    async fn move_file(
        &self,
        from: &str,
        to: &str,
        from_version: ChunkVersion,
        to_version: ChunkVersion,
    ) -> Result<(), String> {
        self.local
            .move_file(from, to, from_version, to_version)
            .await
    }
    async fn move_chunk(&self, from: &str, to: &str) -> Result<(), String> {
        self.local.move_chunk(from, to).await
    }
    async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError> {
        self.local.get_folder_content(path).await
    }
    async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError> {
        self.local.create_folder(path).await
    }
    async fn move_folder(&self, from: &str, to: &str) -> Result<(), String> {
        self.local.move_folder(from, to).await
    }
    async fn delete_chunk(&self, path: &str, version: ChunkVersion) -> Result<(), String> {
        self.local.delete_chunk(path, version).await
    }
}
