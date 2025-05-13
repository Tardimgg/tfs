use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use async_trait::async_trait;
use strum::IntoEnumIterator;
use tokio_stream::iter;
use crate::common::models::{ObjType, PermissionType};
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::PrevSeq;
use crate::services::permission_service::errors::ReadingUserPermissionsToObjectError;
use crate::services::permission_service::fs_helper::FsHelper;
use crate::services::rebac::models::{Permission, RebacKey};
use crate::services::rebac::permission_storage::PermissionStorage;
use crate::services::rebac::rebac::{Rebac, RebacKeyGetter};
use crate::services::user_service::user_service::UserService;
use crate::services::virtual_fs::models::FsNode;
use crate::services::virtual_fs::virtual_fs::VirtualFS;

pub struct PermissionService {
    dht_map: Arc<DHTMap>,
    rebac: Rebac,
    fs_helper: Arc<dyn FsHelper + Send + Sync>
    // тут походу вообще virtualFs нужно будет иметь, тк нужно как то находить папки-предки
    // хотя тут логично придумать что-то третье, к чему будут обращаться и VirtualFs и PermissionService
    // крч замыкания, которые вернут че надо
}

impl PermissionService {
    pub async fn new(dht_map: Arc<DHTMap>, fs_helper: Arc<dyn FsHelper + Send + Sync>, config: Arc<GlobalConfig>) -> Self {

        let anonymous_uid = u64::from_str(&config.get_val(ConfigKey::AnonymousUid).await).unwrap();
        let permissive_permissions = build_permissive_permissions_map(
            fs_helper.clone(),
            anonymous_uid
        );


        let mut additional_permissions = HashMap::new();
        for (permission, childs) in PERMISSION_CHILDS {
            additional_permissions.insert(*permission, childs.to_vec());
        }

        let mut grant_mapping = HashMap::new();
        for permission in PermissionType::iter() {
            if let Some(grant_permission) = to_grant(permission) {
                grant_mapping.insert(permission, grant_permission);
            }
        }

        let rebac_fs_helper = RebacFsHelper {
            fs_helper: fs_helper.clone(),
            rebac_path: config.as_ref().get_val(ConfigKey::RebacPath).await,
        };

        let rebac = Rebac::builder()
            .dht_map(dht_map.clone())
            .config(config.clone())
            .permissive_permissions(permissive_permissions)
            .additional_permissions(additional_permissions)
            .to_grant_mapping(grant_mapping)
            .permission_storage(Arc::new(rebac_fs_helper))
            .build();


        Self { dht_map, rebac, fs_helper}
    }

    pub async fn get_permissions_of_all_users(&self, user_id: u64, obj_type: ObjType,
                                              obj_id: u64) -> Result<Vec<Permission>, ReadingUserPermissionsToObjectError> {
        let key = RebacKey {
            obj_type,
            obj_id,
        };
        let permissions = self.rebac.get_permissions_of_all_users(user_id, key).await?
            .map(|v| v.0)
            .unwrap_or(vec![]);

        Ok(permissions)
    }


    pub async fn get_permissions(&self, user_id: u64, obj_type: ObjType, obj_id: u64) -> Result<Vec<PermissionType>, String> {
        let key = RebacKey {
            obj_type,
            obj_id,
        };
        let permissions = self.rebac.get_user_permissions(user_id, key).await?
            .map(|v| v.0)
            .unwrap_or(vec![]);

        Ok(permissions.into_iter().map(|v| v.permission_type).collect())
    }

    pub async fn check_permission(&self, user_id: u64, obj_type: ObjType, obj_id: u64, permission_type: PermissionType) -> Result<bool, String> {
        let key = RebacKey {
            obj_type,
            obj_id,
        };
        self.rebac.check_permission(user_id, key, permission_type).await
    }

    pub async fn put_permission(&self, current_user: u64, target_user: String, obj_type: ObjType,
                                obj_id: u64, permission_type: PermissionType) -> Result<(), String> {
        let target_uid = self.fs_helper.find_uid(&target_user).await?;
        if let None = target_uid {
            return Err("the user does not exist".to_string())
        }
        let target_uid = target_uid.unwrap();

        let key = RebacKey::new(obj_type, obj_id);
        self.rebac.put_permission(current_user, target_uid, key, permission_type).await
    }

    pub async fn delete_permission(&self, current_user: u64, target_user: String, obj_type: ObjType,
                                obj_id: u64, permission_type: PermissionType) -> Result<(), String> {
        let target_uid = self.fs_helper.find_uid(&target_user).await?;
        if let None = target_uid {
            return Err("the user does not exist".to_string())
        }
        let target_uid = target_uid.unwrap();

        let key = RebacKey::new(obj_type, obj_id);
        self.rebac.delete_permission(current_user, target_uid, key, permission_type).await
    }

    pub async fn put_permission_without_rights_check(&self, target_user: u64, obj_type: ObjType,
                                obj_id: u64, permission_type: PermissionType) -> Result<(), String> {
        let key = RebacKey::new(obj_type, obj_id);
        self.rebac.put_permission_without_rights_check(target_user, key, permission_type).await
    }
}

fn build_permissive_permissions_map(fs_helper: Arc<dyn FsHelper + Send + Sync>, anonymous_uid: u64) -> HashMap<PermissionType, Vec<(Box<dyn RebacKeyGetter + Send + Sync>, PermissionType)>>{
    let mut permissive_permissions: HashMap<PermissionType, Vec<(Box<dyn RebacKeyGetter + Send + Sync>, PermissionType)>> = HashMap::new();

    permissive_permissions.insert(PermissionType::Read, vec![
        (Box::new(AncestorsRebacKeyReceiver {
            fs_helper: fs_helper.clone(),
            obj_type: ObjType::File
        }), PermissionType::ReadRecursively)
    ]);

    permissive_permissions.insert(PermissionType::Write, vec![
        (Box::new(AncestorsRebacKeyReceiver {
            fs_helper: fs_helper.clone(),
            obj_type: ObjType::File
        }), PermissionType::WriteRecursively)
    ]);

    permissive_permissions.insert(PermissionType::ReadPermissionsOfOtherUsers, vec![
        (Box::new(AncestorsRebacKeyReceiver {
            fs_helper: fs_helper.clone(),
            obj_type: ObjType::File
        }), PermissionType::ReadPermissionsOfOtherUsersRecursively)
    ]);

    permissive_permissions.insert(PermissionType::GrantRead, vec![
        (Box::new(AncestorsRebacKeyReceiver {
            fs_helper: fs_helper.clone(),
            obj_type: ObjType::File
        }), PermissionType::GrantReadRecursively)
    ]);

    permissive_permissions.insert(PermissionType::GrantWrite, vec![
        (Box::new(AncestorsRebacKeyReceiver {
            fs_helper: fs_helper.clone(),
            obj_type: ObjType::File
        }), PermissionType::GrantWriteRecursively)
    ]);

    for permission_type in PermissionType::iter() {
        permissive_permissions.entry(permission_type).or_insert(Vec::new()).extend([
            (Box::new(SharedUser {
                uid: anonymous_uid,
            }) as Box<dyn RebacKeyGetter + Send + Sync>, permission_type)
        ]);
    }

    permissive_permissions
}

const PERMISSION_CHILDS: &[(PermissionType, &[PermissionType])] = &[
    (PermissionType::All, &[PermissionType::ReadRecursively, PermissionType::WriteRecursively,
        PermissionType::GrantReadRecursively, PermissionType::GrantWriteRecursively, PermissionType::ReadPermissionsOfOtherUsersRecursively]),
    (PermissionType::ReadRecursively, &[PermissionType::Read]),
    (PermissionType::WriteRecursively, &[PermissionType::Write]),
    (PermissionType::GrantReadRecursively, &[PermissionType::GrantRead]),
    (PermissionType::GrantWriteRecursively, &[PermissionType::GrantWrite]),
    (PermissionType::ReadPermissionsOfOtherUsersRecursively, &[PermissionType::ReadPermissionsOfOtherUsers])
];

struct SharedUser {
    uid: u64,
}

#[async_trait(?Send)]
impl RebacKeyGetter for SharedUser {
    async fn get(&self, uid: u64, rebac_key: RebacKey) -> Result<(u64, Vec<RebacKey>), String> {
        if uid == self.uid {
            return Ok((uid, vec![]))
        }
        Ok((self.uid, vec![rebac_key]))
    }
}

struct AncestorsRebacKeyReceiver {
    fs_helper: Arc<dyn FsHelper + Send + Sync>,
    obj_type: ObjType
}

#[async_trait(?Send)]
impl RebacKeyGetter for AncestorsRebacKeyReceiver {
    async fn get(&self, uid: u64, rebac_key: RebacKey) -> Result<(u64, Vec<RebacKey>), String> {
        let ancestor_objects = self.fs_helper.get_ancestor_objects(rebac_key.obj_id).await?;
        Ok((uid, ancestor_objects.into_iter().map(|v| RebacKey::new(ObjType::Folder, v)).collect()))
    }
}

struct RebacFsHelper {
    fs_helper: Arc<dyn FsHelper + Send + Sync>,
    rebac_path: String
}

#[async_trait(?Send)]
impl PermissionStorage for RebacFsHelper {
    async fn save(&self, permission: Permission, prev_seq: PrevSeq) -> Result<(), String> {
        let id = format!("{}_{}_{}_{}", permission.subject, permission.object.obj_id,
                         permission.object.obj_type.to_str_key(), permission.permission_type.to_str_key());
        let path = format!("{}/{}", &self.rebac_path, id);

        self.fs_helper.save(&path, FsNode::RebacPermission(permission), prev_seq).await
    }
}

const fn to_grant(permission: PermissionType) -> Option<PermissionType> {
    Some(match permission {
        PermissionType::Read => PermissionType::GrantRead,
        PermissionType::Write => PermissionType::GrantWrite,
        PermissionType::GrantRead => PermissionType::GrantRead,
        PermissionType::GrantReadRecursively => PermissionType::GrantReadRecursively,
        PermissionType::GrantWrite => PermissionType::GrantWrite,
        PermissionType::All => PermissionType::All,
        PermissionType::ReadRecursively => PermissionType::GrantReadRecursively,
        PermissionType::WriteRecursively => PermissionType::GrantWriteRecursively,
        PermissionType::GrantWriteRecursively => PermissionType::GrantWriteRecursively,
        PermissionType::ReadPermissionsOfOtherUsers => PermissionType::ReadPermissionsOfOtherUsers,
        PermissionType::ReadPermissionsOfOtherUsersRecursively => PermissionType::ReadPermissionsOfOtherUsersRecursively
    })
}