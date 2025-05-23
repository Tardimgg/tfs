
use actix_web::{ResponseError, web};
use async_stream::__private::AsyncStream;
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, TryStreamExt};
use futures::FutureExt;
use reqwest::StatusCode;
use tokio::fs::File;
use tokio::io::{BufReader, Take};
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_util::io::ReaderStream;
use typed_builder::TypedBuilder;
use crate::common::file_range::FileRange;
use crate::services::file_storage::errors::*;
use crate::services::file_storage::model::{ChunkVersion, FolderMeta, FileSaveOptions};
use crate::services::virtual_fs::models::StoredFile;

pub enum FileStream {
    Payload(web::Payload),
    ReceiverStream(BroadcastStream<Result<Bytes, u16>>),
    TokioFile(ReaderStream<Take<BufReader<File>>>),
    StringData(Option<String>),
    DownloadStream(Box<dyn Stream<Item=Result<Bytes, reqwest::Error>> + Unpin>)
}


impl FileStream {
    pub async fn next(&mut self) -> Option<Result<bytes::Bytes, u16>> {
        match self {
            FileStream::Payload(payload) => payload.next().await
                .map(|v| v.map_err(|err| err.status_code().as_u16())),

            FileStream::TokioFile(stream) => stream.next().await
                .map(|v| v.map_err(|err| err.status_code().as_u16())),
            FileStream::StringData(string) => string.take().map(|v| Ok(v.into())),
            FileStream::ReceiverStream(v) => v.next().await
                .map(|v|
                    v.unwrap_or_else(|err| Err(0))
                ),
            FileStream::DownloadStream(v) => v.next().await
                .map(|v| v.map_err(|err| err.status().map(|s| s.as_u16()).unwrap_or(0)))
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
    async fn save_chunk(&self, path: &str, data: FileStream, options: FileSaveOptions) -> Result<u64, FileSavingError>;
    async fn save(&self, path: &str, range: FileRange, data: FileStream, options: FileSaveOptions) -> Result<u64, FileSavingError>;
    // нужно научится хранить id версии хранимого куска
    async fn get_file<'a>(&self, path: &str, ranges: Option<&'a [FileRange]>, version: Option<ChunkVersion>) -> Result<Vec<(&'a FileRange, FileStream)>, FileReadingError>;
    async fn get_file_meta(&self, path: &str) -> Result<Vec<StoredFile>, FileReadingError>;
    async fn move_file(&self, from: &str, to: &str, from_version: ChunkVersion, to_version: ChunkVersion) -> Result<(), String>;
    async fn move_chunk(&self, from: &str, to: &str) -> Result<(), String>;
    async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError>;
    async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError>;
    async fn move_folder(&self, from: &str, to: &str) -> Result<(), String>;
    async fn delete_chunk(&self, path: &str, version: ChunkVersion) -> Result<(), String>;

}

