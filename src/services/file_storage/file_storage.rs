use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{Context, Poll};
use actix_web::{ResponseError, web};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, TryStreamExt};
use futures::FutureExt;
use tokio::fs::File;
use tokio::io::BufReader;
use tokio_util::io::ReaderStream;
use crate::services::file_storage::errors::*;
use crate::services::virtual_fs::models::FolderMeta;

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

#[async_trait(?Send)]
pub trait FileStorage {

    async fn save(&self, path: &str, data: FileStream) -> Result<(), FileSavingError>;
    async fn get_file(&self, path: &str) -> Result<FileStream, FileReadingError>;
    async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError>;
    async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError>;
}

