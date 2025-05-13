use std::collections::HashSet;
use async_trait::async_trait;
use futures::future::join_all;
use log::error;
use tokio_stream::StreamExt;
use crate::services::dht_map::model::PrevSeq;
use crate::services::rebac::models::Permission;
use crate::services::virtual_fs::models::{FsNode, FsNodeType, GlobalFileInfo, StoredFile};
use crate::services::virtual_fs::virtual_fs::VirtualFS;

#[async_trait(?Send)]
pub trait FsHelper {

    async fn get_ancestor_objects(&self, obj: u64) -> Result<Vec<u64>, String>;

    async fn save(&self, path: &str, fs_node: FsNode, prev_seq: PrevSeq) -> Result<(), String>;

    async fn find_uid(&self, login: &str) -> Result<Option<u64> , String>;
}

#[async_trait(?Send)]
impl FsHelper for VirtualFS {

    async fn get_ancestor_objects(&self, obj: u64) -> Result<Vec<u64>, String> {
        let ancestor_folders = self.get_ancestor_folders(obj).await?;
        if ancestor_folders.is_none() {
            return Ok(vec![]);
        }
        let ancestor_folders = ancestor_folders.unwrap();

        let futures: Vec<_> = ancestor_folders.into_iter()
            .map(async |v| self.get_node_meta(&v).await.map(|v| v.id))
            .collect();

        let ids = join_all(futures).await.into_iter()
            .filter(Result::is_ok)
            .map(Result::unwrap)
            .collect();

        Ok(ids)
    }

    async fn save(&self, path: &str, fs_node: FsNode, prev_seq: PrevSeq) -> Result<(), String> {
        let (locked_version, dht_info) = self.lock_next_file_version(path, fs_node.get_type()).await?;
        let save_node_result = self.save_node(path, &fs_node, locked_version, prev_seq).await;

        let mut lock = HashSet::new();
        lock.insert(StoredFile {
            version: locked_version,
            is_keeper: false
        });
        self.update_state(path, dht_info.map(|v| v.id), fs_node.get_type(), &HashSet::with_capacity(0), &lock, PrevSeq::Any)
            .await.err().iter().for_each(|v| error!("information about the new data could not be saved to dht. Err = {}", v));

        save_node_result
    }

    async fn find_uid(&self, login: &str) -> Result<Option<u64>, String> {
        self.uid_by_login(login).await
    }
}