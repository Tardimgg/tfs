use super::VirtualFS;
use crate::common::common::read_stream_to_string;
use crate::common::models::{Role, UserInfo};
use crate::common::state_update::StateUpdate;
use crate::config::global_config::ConfigKey;
use crate::services::dht_map::dht_map::StrictDhtError;
use crate::services::file_storage::errors::{FileReadingError, FileSavingError};
use crate::services::file_storage::file_storage::FileStream;
use crate::services::file_storage::model::{ChunkVersion, FileSaveOptions};
use crate::services::virtual_fs::models::{
    FileKeeper, FsNode, FsNodeType, GlobalFileInfo, StoredFile,
};
use std::path::{Component, Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RoleGrantError {
    #[error("strict DHT operation failed: {0}")]
    Dht(#[from] StrictDhtError),
    #[error(transparent)]
    Read(#[from] FileReadingError),
    #[error(transparent)]
    Save(#[from] FileSavingError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("user record missing or not a UserInfo record")]
    MissingUser,
    #[error("user identity differs from expected uid/login")]
    IdentityMismatch,
    #[error("user metadata is invalid or contains an unfinished write")]
    InvalidMetadata,
    #[error("committed user version {0} is not available locally")]
    LocalVersionUnavailable(u64),
    #[error("user blob version cannot be incremented")]
    VersionOverflow,
    #[error("publication/readback not confirmed; blob and reservation retained")]
    Unconfirmed,
    #[error("user blob read failed: {0}")]
    BlobRead(String),
    #[error("keeper state notification failed: {0}")]
    Notification(#[from] tokio::sync::mpsc::error::SendError<StateUpdate>),
}

impl VirtualFS {
    pub(crate) async fn uid_by_login_for_role_check(&self, login: &str) -> Result<Option<u64>, StrictDhtError> {
        let path = self.config.get_val(ConfigKey::UserUidMappingPath).await;
        Ok(self.dht_map.get_strict::<u64>(&format!("{path}/{login}")).await?
            .map(|(uid, _)| uid))
    }

    pub async fn user_for_role_check(&self, path: &str) -> Result<UserInfo, RoleGrantError> {
        let (metadata, _) = self.dht_map.get_strict::<GlobalFileInfo>(path).await?
            .ok_or(RoleGrantError::MissingUser)?;
        let version = committed_user_version(path, &metadata)?;
        self.read_committed_user(path, &metadata, version).await
    }

    pub async fn add_user_roles(
        &self,
        path: &str,
        expected_uid: u64,
        expected_login: &str,
        requested_roles: &[Role],
    ) -> Result<(), RoleGrantError> {
        let (metadata, sequence) = self
            .dht_map
            .get_strict::<GlobalFileInfo>(path)
            .await?
            .ok_or(RoleGrantError::MissingUser)?;
        let version = committed_user_version(path, &metadata)?;
        let mut user = self.read_committed_user(path, &metadata, version).await?;
        if user.uid != expected_uid || user.login != expected_login {
            return Err(RoleGrantError::IdentityMismatch);
        }
        if !merge_roles(&mut user, requested_roles) {
            self.confirm_user_metadata(path, &metadata, sequence)
                .await?;
            return Ok(());
        }

        let next_version = version
            .checked_add(1)
            .ok_or(RoleGrantError::VersionOverflow)?;
        let mut reserved = metadata.clone();
        let reservation = StoredFile {
            version: next_version,
            is_keeper: false,
        };
        if let Some(keeper) = reserved
            .keepers
            .iter_mut()
            .find(|keeper| keeper.id == self.id)
        {
            keeper.data.push(reservation);
        } else {
            reserved.keepers.push(FileKeeper {
                id: self.id,
                data: vec![reservation],
            });
        }
        let reservation_sequence = self
            .dht_map
            .compare_exchange_strict(path, (&metadata, sequence), &reserved)
            .await?;
        let post_lock_sequence = self
            .confirm_user_metadata(path, &reserved, reservation_sequence)
            .await?;

        let expected_user = user.clone();
        let json = serde_json::to_string(&FsNode::UserInfo(user))?;
        self.storage
            .save_chunk(
                super::local_path(path),
                FileStream::StringData(Some(json)),
                FileSaveOptions::builder()
                    .version(ChunkVersion(next_version))
                    .build(),
            )
            .await?;

        let mut committed = reserved.clone();
        for keeper in &mut committed.keepers {
            if keeper.id == self.id {
                for stored in &mut keeper.data {
                    if *stored == reservation {
                        stored.is_keeper = true;
                    }
                }
            }
        }
        let published_sequence = self
            .dht_map
            .compare_exchange_strict(path, (&reserved, post_lock_sequence), &committed)
            .await?;
        self.confirm_user_metadata(path, &committed, published_sequence)
            .await?;
        let confirmed_user = self
            .read_committed_user(path, &committed, next_version)
            .await?;
        if confirmed_user != expected_user {
            return Err(RoleGrantError::Unconfirmed);
        }
        self.confirm_user_metadata(path, &committed, published_sequence)
            .await?;
        self.state_updater
            .send(StateUpdate::NewUser(super::local_path(path).to_string()))
            .await?;
        Ok(())
    }

    async fn confirm_user_metadata(
        &self,
        path: &str,
        expected: &GlobalFileInfo,
        sequence: i64,
    ) -> Result<i64, RoleGrantError> {
        match self.dht_map.get_strict::<GlobalFileInfo>(path).await? {
            Some((observed, actual_sequence))
                if observed == *expected && actual_sequence == sequence =>
            {
                Ok(actual_sequence)
            }
            Some(_) | None => Err(RoleGrantError::Unconfirmed),
        }
    }

    async fn read_committed_user(
        &self,
        path: &str,
        metadata: &GlobalFileInfo,
        version: u64,
    ) -> Result<UserInfo, RoleGrantError> {
        if !metadata.keepers.iter().any(|keeper| {
            keeper.id == self.id && keeper.data.contains(&StoredFile { version, is_keeper: true })
        }) {
            return Err(RoleGrantError::LocalVersionUnavailable(version));
        }
        let stream = self.get_file_version(path, version).await?;
        let json = read_stream_to_string(stream.get_stream())
            .await
            .map_err(RoleGrantError::BlobRead)?;
        match serde_json::from_str::<FsNode>(&json)? {
            FsNode::UserInfo(user) => Ok(user),
            FsNode::File(_) | FsNode::Folder(_) | FsNode::RebacPermission(_) => {
                Err(RoleGrantError::MissingUser)
            }
        }
    }

    async fn get_file_version(
        &self,
        path: &str,
        version: u64,
    ) -> Result<FileStream, FileReadingError> {
        let path = super::local_path(path);
        if path.is_empty()
            || !Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(FileReadingError::BadRequest);
        }
        let mut files = self
            .storage
            .get_file(path, None, Some(ChunkVersion(version)))
            .await?;
        if files.len() != 1 {
            return Err(FileReadingError::InternalError(
                "exact-version read did not return one blob".into(),
            ));
        }
        Ok(files.swap_remove(0).1)
    }
}

fn committed_user_version(path: &str, metadata: &GlobalFileInfo) -> Result<u64, RoleGrantError> {
    if metadata.filename != path
        || metadata.file_type != FsNodeType::User
        || metadata
            .keepers
            .iter()
            .flat_map(|keeper| &keeper.data)
            .any(|stored| !stored.is_keeper)
    {
        return Err(RoleGrantError::InvalidMetadata);
    }
    metadata
        .keepers
        .iter()
        .flat_map(|keeper| &keeper.data)
        .map(|stored| stored.version)
        .max()
        .ok_or(RoleGrantError::InvalidMetadata)
}

fn merge_roles(user: &mut UserInfo, requested: &[Role]) -> bool {
    let mut changed = false;
    for role in requested {
        if !user.roles.contains(role) {
            user.roles.push(*role);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests;
