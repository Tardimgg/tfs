pub mod models;

use typed_builder::TypedBuilder;
use crate::services::distributed_map::DistributedMap;
use crate::services::file_storage::FileStorage;
use crate::services::virtual_fs::models::{FileMeta, FolderMeta};

#[derive(TypedBuilder)]
pub struct VirtualFS {
    map: Box<dyn DistributedMap + Send + Sync>,
    storage: Box<dyn FileStorage + Send + Sync>
}

impl VirtualFS {

    pub fn get_folder_content(&self, path: &str) -> FolderMeta {
        FolderMeta::builder()
            .files(vec![])
            .build()
    }

    pub fn get_file_content(&self, path: &str) {

    }

    pub fn create_folder(&self, path: &str) -> FileMeta {
        FileMeta::builder().name("".to_string()).build()

    }

    pub fn create_file(&self, path: &str, data: String) -> FileMeta {
        FileMeta::builder().name("".to_string()).build()
    }
}