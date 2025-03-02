use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::Error;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use actix_web::ResponseError;
use async_trait::async_trait;
use futures::StreamExt;
use tokio::io::{AsyncWriteExt, BufReader, BufWriter};
use tokio_util::io::{ReaderStream, StreamReader};
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::services::file_storage::errors::{CreateFolderError, FileReadingError, FileSavingError, FolderReadingError};
use crate::services::file_storage::file_storage::{FileStorage, FileStream};
use crate::services::file_storage::local_storage::models::{ChunkFilename, Event};
use crate::services::file_storage::model::{ChunkVersion, FileMeta, FolderMeta, NodeType};
use crate::services::virtual_fs::models::{StoredFileRange, StoredFileRangeEnd};

pub struct LocalStorage {
    pub base_path: String
}


impl LocalStorage {
    // добавить перезапись пути, с добавлением суффикам -folder к обычным папкам и суффикса -chunks к папке с чанками

    fn filepath(&self, path: &str) -> PathBuf {
        PathBuf::from(format!("{}/{}-chunks", self.base_path.as_str(), path))
    }

    fn range_filepath(&self, path: &str, range: FileRange, version: ChunkVersion) -> PathBuf {
        let chunk_filename = ChunkFilename::from(range, version);
        self.chunk_filepath(path, chunk_filename)
    }

    fn chunk_filepath(&self, path: &str, chunk_filename: ChunkFilename) -> PathBuf {
        PathBuf::from(format!("{}/{}-chunks/{}", self.base_path.as_str(), path, String::from(chunk_filename)))
    }

    fn folder_path(&self, path: &str) -> String {
        format!("{}/{}", self.base_path.as_str(), path)
    }
}

#[async_trait(?Send)]
// #[async_trait]
impl FileStorage for LocalStorage {
    async fn save(&self, path: &str, range: FileRange, version: ChunkVersion, mut data: FileStream) -> Result<(), FileSavingError> {
        let chunks_folder = self.filepath(path);
        let _ = tokio::fs::create_dir(&chunks_folder).await;

        let filepath = self.range_filepath(path, range, version);
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
    // добавить возможность указать точную версию, ну и вообще подумать на консистентностью таких штук

    async fn get_file<'a>(&self, path: &str, ranges_o: Option<&'a [FileRange]>, max_version: Option<ChunkVersion>) -> Result<Vec<(FileRange, FileStream)>, FileReadingError> {
        if let Some(ranges) = ranges_o {
            if !check_ranges(ranges) {
                return Err(FileReadingError::BadRequest)
            }
        }

        let chunks_folder = self.filepath(path);

        let mut events = if let Some(req_ranges) = ranges_o {
            req_ranges.into_iter().flat_map(|req| vec![Event::StartRequest(req.0), Event::EndRequest(req.1)]).collect()
            // а если endRequest це конец?
        } else {
            vec![Event::StartRequest(0), Event::EndRequest(EndOfFileRange::LastByte)]
        };

        // let mut exist_ranges = Vec::new(); // тут будут проблемы при одновременно удалении (хотя зачем нужно что удалять пока непонятно)
        let mut exist_files = tokio::fs::read_dir(&chunks_folder).await.unwrap();
        while let Ok(file_o) = exist_files.next_entry().await {
            if let Some(file) = file_o {
                if let Some(filename) =  file.file_name().to_str() {
                    if let Ok(chunk_filename) = ChunkFilename::try_from(filename) {

                        let chunk_version = chunk_filename.get_version();
                        if *chunk_version <= max_version.unwrap_or(ChunkVersion(u64::MAX)) {
                            events.push(Event::StartExist(*chunk_filename.get_start(), *chunk_version, chunk_filename));

                            let end = match chunk_filename {
                                ChunkFilename::All(_) => EndOfFileRange::LastByte,
                                ChunkFilename::Range(_, to, _) => to
                            };
                            events.push(Event::EndExist(end, *chunk_version, chunk_filename));
                            // exist_ranges.push(chunk_filename);
                        }
                    }
                }
            } else {
                break;
            }
        }



        events.sort_unstable_by(|e1, e2| {
            let e1_key = get_sort_key(e1);
            let e2_key = get_sort_key(e2);

            let mut res = e1_key.cmp(&e2_key);
            match (e1, e2) {
                (Event::EndExist(_, v, _), Event::EndExist(_, v2, _)) => {
                    res = res.then(v.cmp(&v2));
                }
                _ => {}
            }
            return res;
        });
        println!("{:?}", events);


        // !!!!!!!!!!!!!!!!!!!! нужно убрать пересечения из запроса

        let ranges = calculate_sources_for_ranges(&events)?;

        type StreamType = Result<(FileRange, FileStream), FileReadingError>;
        let streams: Vec<StreamType> = futures::stream::iter(ranges.into_iter())
            .map(|range| async move {
                let file = tokio::fs::File::open(self.chunk_filepath(path, range.0)).await.map_err(|v| FileReadingError::Retryable)?;

                // крч нужно тут чинить, чтобы читатель стал send + sync? (sync наверное не нужно)
                // tx rx?? (но тогда источник будет горячим, никакого обратного давления)
                let buf = BufReader::with_capacity(64 * 1024, file);
                let reader = ReaderStream::new(buf);

                // streams.push(((*range.0.get_start(), *range.0.get_end()), FileStream::TokioFile(reader)));
                Ok(((*range.0.get_start(), *range.0.get_end()), FileStream::TokioFile(reader)))
            })
            .buffered(10)
            .collect()
            .await;

        // let filepath = self.old_filepath(path, 0);


        if streams.iter().all(|v| v.is_ok()) {
            Ok(streams.into_iter().map(|v| v.unwrap()).collect())
        } else {
            Err(FileReadingError::NotExist)
        }


        // Ok(vec![((0, EndOfFileRange::EndOfFile), FileStream::TokioFile(reader))])
    }

    async fn get_file_meta(&self, path: &str) -> Result<Vec<StoredFileRange>, FileReadingError> {
        let chunks_folder = self.filepath(path);
        let mut meta = Vec::new();

        let mut exist_files = tokio::fs::read_dir(&chunks_folder).await.unwrap();
        while let Ok(Some(file)) = exist_files.next_entry().await {
            if let Some(filename) =  file.file_name().to_str() {
                if let Ok(chunk_filename) = ChunkFilename::try_from(filename) {

                    let chunk_version = chunk_filename.get_version();

                    let end = match chunk_filename {
                        ChunkFilename::All(_) => StoredFileRangeEnd::EnfOfFile(file.metadata().await.unwrap().len()),
                        ChunkFilename::Range(from, to, _) => {
                            match to {
                                EndOfFileRange::LastByte => StoredFileRangeEnd::EndOfRange(from + file.metadata().await.unwrap().len()),
                                EndOfFileRange::ByteIndex(byte_index) => StoredFileRangeEnd::EndOfRange(byte_index)
                            }
                        }
                    };

                    let stored_range = StoredFileRange {
                        from: *chunk_filename.get_start(),
                        to: end,
                        version: chunk_version.0,
                    };
                    // meta.push((*chunk_version, (*chunk_filename.get_start(), end)));
                    meta.push(stored_range);
                }
            }
        }
        Ok(meta)
    }

    async fn move_file(&self, from: &str, to: &str) -> Result<(), String> {
        todo!()
    }

    async fn get_folder_content(&self, path: &str) -> Result<FolderMeta, FolderReadingError> {
        let folder_path = self.folder_path(path);
        let mut dir = tokio::fs::read_dir(folder_path).await.map_err(|v| FolderReadingError::NotExist)?;

        let mut files = Vec::new();
        while let Some(entry) = dir.next_entry().await.map_err(|v| FolderReadingError::InternalError(v.to_string()))? {
            let filename = entry.file_name().to_str().unwrap().to_string();
            let mut external_filename: &str = &filename;
            let mut node_type = NodeType::Folder;
            if filename.ends_with("-chunks") {
                external_filename = &filename[0..filename.len() - 7];
                node_type = NodeType::File;
            }
            let meta = FileMeta::builder()
                .name(external_filename.to_string())
                .node_type(node_type)
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

    async fn move_folder(&self, from: &str, to: &str) -> Result<(), String> {
        todo!()
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

fn calculate_sources_for_ranges(events: &[Event]) -> Result<Vec<(ChunkFilename, (u64, EndOfFileRange))>, FileReadingError> {
    let mut in_request = false;
    let mut ranges: Vec<(ChunkFilename, (u64, EndOfFileRange))> = Vec::new();
    let mut opened_exist = BTreeMap::new();
    let mut already_closed = HashSet::new();

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