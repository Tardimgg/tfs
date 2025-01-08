use std::io::Error;
use std::marker::PhantomData;
use actix_web::http::StatusCode;
use actix_web::ResponseError;
use async_trait::async_trait;
use futures::StreamExt;
use tokio::io::{AsyncWriteExt, BufReader, BufWriter};
use tokio_util::io::{ReaderStream, StreamReader};
use crate::services::file_storage::errors::{CreateFolderError, FileReadingError, FileSavingError, FolderReadingError};
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::virtual_fs::models::{FileMeta, FolderMeta};

pub struct LocalStorage {
    pub base_path: String
}

impl LocalStorage {

    fn filepath(&self, path: &str) -> String{
        format!("{}/{}", self.base_path.as_str(), path)
    }
}

#[async_trait(?Send)]
impl FileStorage for LocalStorage {
    async fn save(&self, path: &str, mut data: FileStream) -> Result<(), FileSavingError> {
        let filepath = self.filepath(path);
        let file = tokio::fs::File::create(filepath).await.map_err(|v| FileSavingError::AlreadyExist)?;
        let mut writer = BufWriter::new(file);

        // writer.write_all_buf()
        while let Some(item) = data.next().await {
            // data.extend_from_slice(&item?);
            // println!("{:?}", item)
            // writer.write_all(format!("{:?}", item).as_str().as_bytes()).await;
            writer.write_all(item.unwrap().as_ref()).await.unwrap();
        }
        writer.flush().await.unwrap();
        writer.get_ref().sync_all().await.unwrap();
        Ok(())
        // file.write_all_buf(&mut data);
        // tokio::fs::write("my_file.bin", data)
    }

    async fn get_file(&self, path: &str) -> Result<FileStream, FileReadingError> {
        let filepath = self.filepath(path);
        let file = tokio::fs::File::open(filepath).await.map_err(|v| FileReadingError::NotExist)?;
        // let file = tokio::fs::File::open(filepath).await.unwrap();
        // file.set_max_buf_size(10);

        let buf = BufReader::with_capacity(64 * 1024, file);
        // let mut reader = ReaderStream::new(BufReader::new(file));
        let mut reader = ReaderStream::new(buf);

        // let v = reader.next().await;
        // println!("{:?}",v);

        Ok(FileStream::TokioFile(reader))
    }

    async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError> {
        let folder_path = self.filepath(path);
        let mut dir = tokio::fs::read_dir(folder_path).await.map_err(|v| FolderReadingError::NotExist)?;

        let mut files = Vec::new();
        while let Some(entry) = dir.next_entry().await.map_err(|v| FolderReadingError::InternalError(v.to_string()))? {
            let meta = FileMeta::builder()
                .name(entry.file_name().to_str().unwrap().to_string())
                .build();

            files.push(meta);
        }

        Ok(FolderMeta::builder()
            .files(files)
            .build())
    }

    async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError> {
        tokio::fs::create_dir(self.filepath(path)).await.map_err(|v| CreateFolderError::AlreadyExist)
        // let dir = tokio::fs::read_dir(path).await.map_err(|e| e.to_string())?;
        // di
    }
}