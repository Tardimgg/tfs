pub mod models;

use std::any::Any;
use std::fmt::Debug;
use actix_web::body::MessageBody;
use actix_web::web;
use bytes::Bytes;
use futures::{FutureExt, Stream, StreamExt, TryStreamExt};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio_util::io::ReaderStream;
use typed_builder::TypedBuilder;
use crate::common::buffered_consumer::BufferedConsumer;
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::error::{DhtGetError, DhtPutError};
use crate::services::file_storage::errors::*;
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::virtual_fs::models::{FileMeta, FolderMeta};

#[derive(TypedBuilder)]
pub struct VirtualFS {
    map: Box<DHTMap>,
    storage: Box<dyn FileStorage + Send + Sync>,

    #[builder(default="root".to_string())]
    base_path: String
}

impl VirtualFS {

    pub fn new(map: Box<DHTMap>, storage: Box<dyn FileStorage + Send + Sync>) -> Self {
        VirtualFS {
            map, storage,
            base_path: "root".to_string(),
        }
    }

    pub async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError> {
        self.storage.get_folder_content(path).await

        // переделать на схему врапперов итераторов  VirtualFS() // нинада
    }

    pub async fn get_file_content(&self, path: &str) -> Result<FileStream, FileReadingError> {
        self.storage.get_file(path).await
    }

    pub async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError> {
        self.storage.create_folder(path).await

    }
    pub async fn create_file(&self, path: &str, mut data: web::Payload) -> Result<FileMeta, FileSavingError> {
        self.storage.save(path, FileStream::Payload(data.into())).await?;
        Ok(FileMeta::builder().name(path.to_string()).build())
    }

}