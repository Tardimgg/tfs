use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{Context, Poll};
use actix_web::{ResponseError, web};
use async_stream::stream;
use async_trait::async_trait;
use futures::{Stream, StreamExt, TryStreamExt};
use futures::FutureExt;
use tokio::fs::File;
use tokio::io::BufReader;
use tokio_util::io::ReaderStream;
use crate::common::file_range::FileRange;
use crate::services::file_storage::errors::*;
use crate::services::file_storage::model::{ChunkVersion, FolderMeta};
use crate::services::virtual_fs::models::StoredFileRange;

pub enum FileStream {
    Payload(web::Payload),
    TokioFile(ReaderStream<BufReader<File>>)
}


impl FileStream {
    pub async fn next(&mut self) -> Option<Result<bytes::Bytes, u16>> {
        match self {
            FileStream::Payload(payload) => payload.next().await
                .map(|v| v.map_err(|err| err.status_code().as_u16())),

            FileStream::TokioFile(stream) => stream.next().await
                .map(|v| v.map_err(|err| err.status_code().as_u16()))
        }
    }

    pub fn get_stream(mut self) -> impl Stream<Item=Result<bytes::Bytes, String>> {
        stream! {
            while let Some(v) = self.next().await {
                yield v.map_err(|err| err.to_string());
            }
        }
    }
}

// #[async_trait(?Send)]
#[async_trait(?Send)]
pub trait FileStorage {
// нужно как то построить заборы над путями, может же придти совсем рандомный путь куда угодно
    async fn save(&self, path: &str, range: FileRange, version: ChunkVersion, data: FileStream) -> Result<(), FileSavingError>;
    // нужно научится хранить id версии хранимого куска
    async fn get_file<'a>(&self, path: &str, ranges: Option<&'a [FileRange]>, max_version: Option<ChunkVersion>) -> Result<Vec<(FileRange, FileStream)>, FileReadingError>;
    async fn get_file_meta(&self, path: &str) -> Result<Vec<StoredFileRange>, FileReadingError>;
    async fn move_file(&self, from: &str, to: &str) -> Result<(), String>;
    async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError>;
    async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError>;
    async fn move_folder(&self, from: &str, to: &str) -> Result<(), String>;
}

