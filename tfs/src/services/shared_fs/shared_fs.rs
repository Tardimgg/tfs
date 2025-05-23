use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use actix_web::http::header::Range;
use actix_web::web;
use futures::future::join_all;
use typed_builder::TypedBuilder;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::common::default_error::DefaultError;
use crate::common::exceptions::NodeMetaReceivingError;
use crate::common::models::{FolderContent, FolderContentE, ObjType, PermissionType};
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::file_storage::errors::{ChunkSavingExistingError, CreateFolderError, FileDeletingError, FileReadingError, FileSavingError, FolderReadingError};
use crate::services::file_storage::file_storage::FileStream;
use crate::services::file_storage::model::{FileMeta, FolderMeta};
use crate::services::permission_service::permission_service::PermissionService;
use crate::services::virtual_fs::models::{File, FsNodeType, GlobalFileInfo, StoredFile};
use crate::services::virtual_fs::virtual_fs::VirtualFS;

#[derive(TypedBuilder)]
pub struct SharedFS {
    virtual_fs: Arc<VirtualFS>,
    permission_service: Arc<PermissionService>,
    config: Arc<GlobalConfig>
}

impl SharedFS {

    pub async fn get_folder_content(&self, current_user: AuthenticatedUser, path: &str) -> Result<FolderContentE, FolderReadingError> {
        let folder = self.virtual_fs.get_folder_content(path).await;

        if let Err(err) = folder {
            return match err {
                FolderReadingError::NotExist(_) => Err(FolderReadingError::AccessDenied),
                FolderReadingError::InternalError(v) => Err(FolderReadingError::InternalError(v)),
                FolderReadingError::BadRequest(v) => Err(FolderReadingError::BadRequest(v)),
                FolderReadingError::AccessDenied => Err(FolderReadingError::AccessDenied),
            }
        }
        let folder = folder?;

        if self.permission_service.check_permission(current_user.uid, ObjType::Folder, folder.id, PermissionType::Read).await? {
            Ok(FolderContentE {
                name: folder.name,
                files: folder.files,
                is_partial: false,
                id: folder.id
            })

        } else {
            let file_permissions_tasks: Vec<_> = folder.files.iter()
                .map(async |file| {
                    self.get_node_meta(current_user, &format!("{}/{}", path.trim_end_matches("/"), file.name)).await
                })
                .collect();

            let available_files: HashSet<_> = join_all(file_permissions_tasks).await
                .into_iter()
                .filter(Result::is_ok)
                .map(|v| Path::new(&v.unwrap().filename).file_name().unwrap().to_str().unwrap().to_string())
                .collect();

            if available_files.is_empty() {
                Err(FolderReadingError::AccessDenied)
            } else {
                Ok(FolderContentE {
                    name: folder.name,
                    files: folder.files.into_iter()
                        .filter(|v| available_files.contains(&v.name))
                        .collect(),
                    is_partial: true,
                    id: 0
                })
            }

        }
    }

    pub async fn create_folder(&self, current_user: AuthenticatedUser, path: &str) -> Result<(), CreateFolderError> {
        let has_permission = self.check_root_folder(current_user, path, PermissionType::Write)
            .await
            .map_err(|v| CreateFolderError::InternalError(v))?;

        if has_permission {
            self.virtual_fs.create_folder(path).await?;
            Ok(())
        } else {
            Err(CreateFolderError::AlreadyExist)
        }
    }

    pub async fn get_stored_parts_meta(&self, current_user: AuthenticatedUser, path: &str) -> Result<Vec<StoredFile>, String> {
        if !self.user_is_instance_of_service(current_user).await {
            if let Err(err) = self.get_node_meta(current_user, path).await {
                return match err {
                    NodeMetaReceivingError::NotFound => Err("NotExist".to_string()),
                    NodeMetaReceivingError::InternalError(v) => Err(format!("InternalError, {}", v)),
                    NodeMetaReceivingError::AccessDenied => Err("AccessDenied".to_string())
                }
            }
        }
        self.virtual_fs.get_stored_parts_meta(path).await
    }

    pub async fn get_node_meta(&self, current_user: AuthenticatedUser, path: &str) -> Result<GlobalFileInfo, NodeMetaReceivingError> {
        let meta = self.virtual_fs.get_node_meta(path).await?;
        let obj_type = match meta.file_type {
            FsNodeType::Folder => ObjType::Folder,
            FsNodeType::File => ObjType::File,
            _ => ObjType::File
        };
        if self.permission_service.check_permission(current_user.uid, obj_type, meta.id, PermissionType::Read).await? {
            Ok(meta)
        } else {
            Err(NodeMetaReceivingError::AccessDenied)
        }
    }

    pub async fn get_file_meta(&self, current_user: AuthenticatedUser, path: &str) -> Result<Option<File>, FileReadingError> {
        // temp code. data must be processed through a regular pipeline
        let home_path = self.config.get_val(ConfigKey::HomePath).await;
        if !path.starts_with(&home_path) {
            return Err(FileReadingError::BadRequest)
        }
        if let Err(err) = self.get_node_meta(current_user, path).await {
            return match err {
                NodeMetaReceivingError::NotFound => Err(FileReadingError::NotExist),
                NodeMetaReceivingError::InternalError(v) => Err(FileReadingError::InternalError(v)),
                NodeMetaReceivingError::AccessDenied => Err(FileReadingError::AccessDenied)
            }
        }
        self.virtual_fs.get_file_meta(path).await
    }

    pub async fn get_file_content(&self, current_user: AuthenticatedUser, path: &str,
                                  range_o: Option<Range>) -> Result<Option<FileStream>, FileReadingError> {
        // temp code. data must be processed through a regular pipeline
        let data_path = self.config.get_val(ConfigKey::DataPath).await;
        let data_meta_path = self.config.get_val(ConfigKey::DataMetaPath).await;
        if !path.starts_with(&data_path) && !path.starts_with(&data_meta_path) {
            if let Err(err) = self.get_node_meta(current_user, path).await {
                return match err {
                    NodeMetaReceivingError::NotFound => Err(FileReadingError::NotExist),
                    NodeMetaReceivingError::InternalError(v) => Err(FileReadingError::InternalError(v)),
                    NodeMetaReceivingError::AccessDenied => Err(FileReadingError::AccessDenied)
                }
            }
        }
        self.virtual_fs.get_file_content(path, range_o).await
    }

    pub async fn save_existing_chunk(&self, current_user: AuthenticatedUser, path: &str,
                                     mut data: web::Payload, range_o: Option<Range>,
                                     fs_node_type: FsNodeType, version: u64) -> Result<FileMeta, ChunkSavingExistingError> {
        if self.user_is_instance_of_service(current_user).await {
            Ok(self.virtual_fs.save_existing_chunk(path, data, range_o, fs_node_type, version).await?)
        } else {
            Err(ChunkSavingExistingError::AccessDenied)
        }
    }

    pub async fn put_user_file(&self, current_user: AuthenticatedUser, path: &str,
                               data: FileStream, range_o: Option<Range>) -> Result<FileMeta, FileSavingError> {
        let has_permission = self.check_root_folder(current_user, path, PermissionType::Write)
            .await
            .map_err(|v| FileSavingError::Other(v))?;

        if has_permission {
            self.virtual_fs.put_user_file(path, data, range_o).await
        } else {
            Err(FileSavingError::AccessDenied("".to_string()))
        }
    }

    pub async fn delete_user_file(&self, current_user: AuthenticatedUser, path: &str) -> Result<(), FileDeletingError> {
        let has_permission = self.check_root_folder(current_user, path, PermissionType::Write)
            .await
            .map_err(|v| FileDeletingError::Other(v))?;

        if has_permission {
            self.virtual_fs.delete_user_file(path).await
        } else {
            Err(FileDeletingError::AccessDenied("".to_string()))
        }
    }

    async fn check_root_folder(&self, current_user: AuthenticatedUser, path: &str,
                               permission_type: PermissionType) -> Result<bool, String> {
        let mut path_buf = PathBuf::from(path);
        path_buf.pop();

        let root_node = self.get_node_meta(current_user, path_buf.to_str().unwrap())
            .await
            .default_res()?;
        self.permission_service.check_permission(current_user.uid, ObjType::Folder, root_node.id, permission_type).await
    }

    async fn user_is_instance_of_service(&self, current_user: AuthenticatedUser) -> bool {
        let service_uid_string = self.config.get_val(ConfigKey::SelfUid).await;
        let service_uid = u64::from_str(&service_uid_string).unwrap();
        current_user.uid == service_uid
    }
}