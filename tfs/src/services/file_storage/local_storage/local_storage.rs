use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{Error, SeekFrom};
use std::marker::PhantomData;
use std::ops::Bound::Included;
use std::path::{Path, PathBuf};
use actix::WrapStream;
use actix_web::ResponseError;
use async_stream::__private::AsyncStream;
use async_stream::{stream, try_stream};
use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use futures::{Stream, StreamExt};
use futures::future::join_all;
use tokio::io::{AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::pin;
use tokio_util::io::{ReaderStream, StreamReader};
use crate::common::default_error::DefaultError;
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::services::file_storage::errors::{CreateFolderError, FileReadingError, FileSavingError, FolderReadingError};
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::file_storage::local_storage::models::{ChunkFilename, Event};
use crate::services::file_storage::model::{ChunkVersion, FileMeta, FolderMeta, NodeType, FileSaveOptions};
use crate::services::virtual_fs::models::{StoredFile, StoredFileRangeEnd};

pub struct LocalStorage {
    pub base_path: String
}


impl LocalStorage {
    // добавить перезапись пути, с добавлением суффикам -folder к обычным папкам и суффикса -chunks к папке с чанками

    fn filepath(&self, path: &str) -> PathBuf {
        PathBuf::from(format!("{}/{}", self.base_path.as_str(), path))
    }

    fn path_to_versioned_file(&self, path: &str, version: ChunkVersion) -> PathBuf {
        let mut path_buf = PathBuf::from(path);
        let chunk_filename = ChunkFilename::from(path_buf.file_name().unwrap().to_str().unwrap(), version);
        path_buf.pop();
        path_buf.push(String::from(chunk_filename));

        let mut from_root = PathBuf::from(self.base_path.as_str());
        from_root.push(path_buf);
        from_root
    }

    fn chunk_filepath(&self, path: &str, chunk_filename: ChunkFilename) -> PathBuf {
        PathBuf::from(format!("{}/{}/{}", self.base_path.as_str(), path, String::from(chunk_filename)))
    }

    fn folder_path(&self, path: &str) -> String {
        format!("{}/{}", self.base_path.as_str(), path)
    }

    async fn read_part_file(&self, file_path: &Path, range: &FileRange) -> Result<FileStream, FileReadingError> {
        let mut file = tokio::fs::File::open(file_path)
            .await
            .map_err(|v| FileReadingError::Retryable(v.to_string()))?;

        file.seek(SeekFrom::Start(range.0)).await.map_err(|e| FileReadingError::InternalError(e.to_string()))?;

        // file.start_seek(SeekFrom::Start(range.0)).default_res()?;
        // крч нужно тут чинить, чтобы читатель стал send + sync? (sync наверное не нужно)
        // tx rx?? (но тогда источник будет горячим, никакого обратного давления)
        let buf = BufReader::with_capacity(64 * 1024, file);
        let buf_take = match range.1 {
            EndOfFileRange::LastByte => buf.take(u64::MAX),
            EndOfFileRange::ByteIndex(index) => buf.take(index - range.0 + 1)
        };
        let reader = ReaderStream::new(buf_take);

        Ok(FileStream::TokioFile(reader))
    }
}

#[async_trait(?Send)]
impl FileStorage for LocalStorage {

    async fn save_chunk(&self, path: &str, data: FileStream, options: FileSaveOptions) -> Result<u64, FileSavingError> {
        self.save(path, (0, EndOfFileRange::LastByte), data, options).await
    }

    async fn save(&self, path: &str, range: FileRange, mut data: FileStream, options: FileSaveOptions) -> Result<u64, FileSavingError> {
        // let chunks_folder = self.filepath(path);
        // let _ = tokio::fs::create_dir(&chunks_folder).await;
        let version = options.version;

        let mut root = self.filepath(path);
        root.pop();
        tokio::fs::create_dir_all(&root).await.default_res()?;

        let filepath = self.path_to_versioned_file(path, version);
        let file = if options.update_if_exists {
            tokio::fs::File::create(filepath.clone()).await
        } else {
            tokio::fs::File::create_new(filepath.clone()).await
        }.map_err(|v| FileSavingError::Other(format!("{}; {:?}", v.to_string(), filepath.to_str())))?;

        let mut writer = BufWriter::new(file);

        let mut real_size = 0;
        while let Some(item) = data.next().await {
            match item {
                Ok(bytes) => {
                    real_size += bytes.len();
                    writer.write_all(bytes.as_ref()).await.unwrap();
                }
                Err(error) => {
                    writer.flush().await.err().iter()
                        .for_each(|e| println!("error when flush the file({}): {}", path, e.to_string()));
                    writer.get_ref().sync_all().await.err().iter()
                        .for_each(|e| println!("error when sync_all() the file(){}: {}", path, e.to_string()));

                    // переделать на записы в /temp и потом проверку необходимости удаления
                    if let Err(e) = tokio::fs::remove_file(filepath).await {
                        println!("file deletion error following a reading error. Error {}", e.to_string())
                    }
                    return Err(FileSavingError::Other(error.to_string()));
                }
            }
        }
        writer.flush().await.err().iter()
            .for_each(|e| println!("error when flush the file({}): {}", path, e.to_string()));
        writer.get_ref().sync_all().await.err().iter()
            .for_each(|e| println!("error when sync_all() the file(){}: {}", path, e.to_string()));

        if let EndOfFileRange::ByteIndex(last_byte) =  range.1 {
            if last_byte - range.0 + 1 != real_size as u64 {
                return Err(FileSavingError::InvalidRange);
            }
        }
        // Ok(writer.get_ref().metadata().await.unwrap().len())
        Ok(real_size as u64)
        // file.write_all_buf(&mut data);
        // tokio::fs::write("my_file.bin", data)
    }
    // добавить возможность указать точную версию, ну и вообще подумать на консистентностью таких штук


    // переделать на RangeBound
    async fn get_file<'a>(&self, path: &str, ranges_o: Option<&'a [FileRange]>, version: Option<ChunkVersion>) -> Result<Vec<(&'a FileRange, FileStream)>, FileReadingError> {
        if let Some(ranges) = ranges_o {
            if !check_ranges(ranges) {
                return Err(FileReadingError::BadRequest)
            }
        }

        let mut root_folder = self.filepath(path);
        let filename = root_folder.file_name().unwrap().to_str().unwrap().to_string();
        root_folder.pop();

        let mut exist_versions = Vec::new();

        // let mut exist_ranges = Vec::new(); // тут будут проблемы при одновременно удалении (хотя зачем нужно что удалять пока непонятно)
        let mut exist_files = tokio::fs::read_dir(&root_folder).await.map_err(|v| FileReadingError::NotExist)?;
        while let Ok(file_o) = exist_files.next_entry().await {
            if let Some(file) = file_o {
                if let Some(versioned_filename) =  file.file_name().to_str() {
                    if let Ok(chunk_filename) = ChunkFilename::try_from(versioned_filename) {
                        if chunk_filename.name == filename {
                            exist_versions.push(chunk_filename)
                        }
                    }
                }
            } else {
                break;
            }
        }
        let final_version = if let Some(target_version) = version {
            exist_versions.iter().find(|v| v.version == target_version)
        } else {
            exist_versions.iter().max_by_key(|v| v.version)
        }.ok_or(FileReadingError::NotExist)?;


        let file_path = root_folder.join(PathBuf::from(String::from(final_version.clone())));
        let file_path_ref = &file_path;

        let res = join_all(ranges_o.unwrap_or(&[(0, EndOfFileRange::LastByte)]).into_iter()
            .map(|range| async move { (range, self.read_part_file(file_path_ref, range).await.unwrap()) })
        ).await;

        Ok(res)
        // streams.push(((*range.0.get_start(), *range.0.get_end()), FileStream::TokioFile(reader)));
        // Ok(((0, EndOfFileRange::LastByte), FileStream::TokioFile(reader)))



        // Ok(vec![((0, EndOfFileRange::EndOfFile), FileStream::TokioFile(reader))])
    }


    async fn get_file_meta(&self, path: &str) -> Result<Vec<StoredFile>, FileReadingError> {
        let mut root_folder = self.filepath(path);
        let filename = root_folder.file_name().unwrap().to_str().unwrap().to_string();
        root_folder.pop();

        let mut meta = Vec::new();

        let mut exist_files = tokio::fs::read_dir(&root_folder).await.map_err(|v| FileReadingError::NotExist)?;
        while let Ok(Some(file)) = exist_files.next_entry().await {
            if let Some(versioned_filename) =  file.file_name().to_str() {
                if let Ok(chunk_filename) = ChunkFilename::try_from(versioned_filename) {
                    if chunk_filename.name == filename {
                        let chunk_version = chunk_filename.version;

                        let stored_range = StoredFile {
                            version: chunk_version.0,
                            is_keeper: true
                        };
                        // meta.push((*chunk_version, (*chunk_filename.get_start(), end)));
                        meta.push(stored_range);
                    }
                }
            }
        }
        Ok(meta)
    }

    async fn move_file(&self, from: &str, to: &str, from_version: ChunkVersion, to_version: ChunkVersion) -> Result<(), String> {
        todo!()
    }

    async fn move_chunk(&self, from: &str, to: &str) -> Result<(), String> {
        tokio::fs::rename(
            self.path_to_versioned_file(from, ChunkVersion(0)),
            self.path_to_versioned_file(to, ChunkVersion(0))
        ).await.default_res()
    }

    async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError> {
        let folder_path = self.folder_path(path);
        let mut dir = tokio::fs::read_dir(&folder_path).await.map_err(|v| FolderReadingError::NotExist(path.to_string()))?;

        let mut files = Vec::new();
        while let Some(entry) = dir.next_entry().await.map_err(|v| FolderReadingError::InternalError(v.to_string()))? {
            let filename = entry.file_name().to_str().unwrap().to_string();
            let mut external_filename;
            let mut node_type = NodeType::Folder;
            if entry.file_type().await.default_res()?.is_file() {
                // external_filename = &filename[0..filename.len() - 7]; // кажется это удаляло '-chunks', когда это было
                external_filename = ChunkFilename::try_from(filename.as_str()).default_res()?.name;
                node_type = NodeType::File;
            } else {
                external_filename = filename;
            }
            let meta = FileMeta::builder()
                .name(external_filename)
                .node_type(node_type)
                .build();

            files.push(meta);
        }

        Ok(FolderMeta::builder()
            .files(files)
            .name(PathBuf::from(folder_path).file_name().unwrap().to_str().unwrap().to_string())
            .build())
    }

    async fn create_folder(&self, path: &str) -> Result<(), CreateFolderError> {
        tokio::fs::create_dir(self.filepath(path)).await.map_err(|v| CreateFolderError::AlreadyExist)
        // let dir = tokio::fs::read_dir(path).await.map_err(|e| e.to_string())?;
        // di
    }

    async fn move_folder(&self, from: &str, to: &str) -> Result<(), String> {
        todo!()
    }

    async fn delete_chunk(&self, path: &str, version: ChunkVersion) -> Result<(), String> {

        let versioned_filename = self.path_to_versioned_file(path, version);
        tokio::fs::remove_file(versioned_filename).await.default_res()
    }
}

fn get_sort_key(event: &Event) -> (u64, u64) {
    match event {
        Event::StartExist(from,  version, id) => (*from, version.0),
        Event::EndExist(to, version_, id) => (*cast_end_of_chunk_to_index(to), u64::MAX),
        Event::StartRequest(from) => (*from, u64::MAX - 1),
        Event::EndRequest(to) => (*cast_end_of_chunk_to_index(to), u64::MAX - 2),
    }
}
fn cast_end_of_chunk_to_index(end_of_file_range: &EndOfFileRange) -> &u64 {
    match end_of_file_range {
        EndOfFileRange::LastByte => &u64::MAX,
        EndOfFileRange::ByteIndex(index) => index
    }
}

// нужно тут все переписать на range bound. range (from..to), не включая to, иначе нормально не обработать ____||____ последовательные сегменты
// идем до end максимальной версии путем блокирования обработки сегментов более старых
/*
fn calculate_sources_for_ranges(events: &[Event]) -> Result<Vec<(ChunkFilename, (u64, EndOfFileRange))>, FileReadingError> {
    let mut in_request = false;
    let mut ranges: Vec<(ChunkFilename, (u64, EndOfFileRange))> = Vec::new();
    let mut opened_exist = BTreeMap::new();
    let mut already_closed = HashSet::new();
    let mut min_version = 0;

    for event in events {
        match event {
            Event::StartExist(from, version, id) => {
                let old_val_o = opened_exist.insert(*version, *id);
                if let Some(old_val) = old_val_o {
                    already_closed.insert(*version);
                }

                if in_request {
                    let new_max_version = opened_exist.last_key_value().unwrap();
                    if let Some(last_range) = ranges.last_mut() {
                        if new_max_version.0 > last_range.0.get_version() {
                            last_range.1.1 = EndOfFileRange::ByteIndex(from - 1);
                            ranges.push((*new_max_version.1, (*from, EndOfFileRange::ByteIndex(0))))
                        }
                    } else {
                        panic!("internal logic error 2");
                    }
                }
            }
            Event::EndExist(end_exist, version, chunk_filename) => {
                if let None = already_closed.get(version) {
                    opened_exist.remove(version);
                } else {
                    already_closed.remove(version);
                }

                if in_request {
                    if let Some(new_max_version) = opened_exist.last_key_value() {
                        if let Some(last_request) = ranges.last_mut() {
                            if last_request.0 == *chunk_filename {
                                last_request.1.1 = *end_exist;
                                if let EndOfFileRange::ByteIndex(prev_end) = end_exist {
                                    if let EndOfFileRange::ByteIndex(new_end) = new_max_version.1.get_end() {
                                        if *new_end < prev_end + 1 {
                                            panic!("internal logic error 4");
                                        }
                                    } else {
                                        panic!("internal logic error 3");
                                    }
                                    ranges.push((*new_max_version.1, (prev_end + 1, EndOfFileRange::ByteIndex(0))))
                                } else {
                                    panic!("internal logic error 2"); // конец файла, при этом почему то всё ещё считаем запрос
                                }
                            }
                        } else {
                            panic!("internal logic error 1");
                        }
                        if opened_exist.len() == 0 {
                            return Err(FileReadingError::NotExist);
                        }
                    } else {
                        panic!("internal logic error 0");
                    }
                }
            }
            Event::StartRequest(from) => {
                if in_request {
                    panic!("internal logic error -1");
                }
                in_request = true;
                if let Some((version, filename)) = opened_exist.last_key_value() {
                    ranges.push((*filename, (*from, EndOfFileRange::ByteIndex(0))))
                } else {
                    panic!("internal logic error -2");
                }
            }
            Event::EndRequest(end) => {
                let range = ranges.last_mut().unwrap();
                range.1.1 = *end;
                in_request = false;
            }
        }
    }
    Ok(ranges)
}

 */

fn check_ranges(ranges: &[FileRange]) -> bool {
    let mut sorted = ranges.to_owned();
    sorted.sort_unstable_by_key(|v| v.0);

    let mut prev_r = EndOfFileRange::ByteIndex(0);

    for range in sorted {
        if range.1 < EndOfFileRange::ByteIndex(range.0) {
            return false
        }
        if range.1 < prev_r {
            return false;
        }
        prev_r = range.1;
    }

    true
}

