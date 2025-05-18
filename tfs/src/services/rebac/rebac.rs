use std::collections::HashMap;
use std::future::Future;
use std::ops::AsyncFn;
use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use const_for::const_for;
use futures::future::join_all;
use futures::{StreamExt, TryFutureExt};
use itertools::Itertools;
use log::{error, warn};
use tryhard::retry_fn;
use typed_builder::TypedBuilder;
use crate::common::default_error::DefaultError;
use crate::common::models::{ObjType, PermissionType};
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::PrevSeq;
use crate::services::rebac::errors::UserPermissionsReadingError;
use crate::services::rebac::models::{Permission, RebacKey};
use crate::services::rebac::permission_storage::PermissionStorage;
use crate::services::virtual_fs::models::GlobalFileInfo;

#[async_trait(?Send)]
pub trait RebacKeyGetter {
    async fn get(&self, uid: u64, rebac_key: RebacKey) -> Result<(u64, Vec<RebacKey>), String>;
}


#[derive(TypedBuilder)]
pub struct Rebac {
    dht_map: Arc<DHTMap>,
    config: Arc<GlobalConfig>,
    permissive_permissions: HashMap<PermissionType, Vec<(Box<dyn RebacKeyGetter + Send + Sync>, PermissionType)>>,
    additional_permissions: HashMap<PermissionType, Vec<PermissionType>>,
    to_grant_mapping: HashMap<PermissionType, PermissionType>,
    permission_storage: Arc<dyn PermissionStorage + Send + Sync>
}

impl Rebac {

    async fn put_permission_impl(&self, target_user: u64, obj_id: RebacKey,
                                 permission_type: PermissionType) -> Result<(), String> {
        let user_permissions = self.get_permissions_of_all_users_without_rights_check(obj_id).await?;
        let mut prev_seq = PrevSeq::Seq(None);
        let mut prev_permissions = Vec::with_capacity(0);
        let mut prev_version = None;
        if let Some((exists_permission, dht_seq)) = user_permissions {
            let user_permissions: Vec<_> = exists_permission.iter()
                .filter(|v| v.permission_type == permission_type && v.subject == target_user)
                .collect();
            if !user_permissions.is_empty() {
                if user_permissions.len() > 1 {
                    error!("user_permissions contains duplicates: {:?}", user_permissions);
                }
                let max_version = user_permissions.iter().max_by_key(|v| v.version).unwrap();
                if !max_version.deleted {
                    return Ok(());
                }
                prev_version = Some(max_version.version)
            }

            prev_seq = PrevSeq::Seq(Some(dht_seq));
            prev_permissions = exists_permission;
        }

        let permission = Permission {
            subject: target_user,
            object: obj_id,
            permission_type,
            version: prev_version.map(|v| v + 1).unwrap_or(0),
            deleted: false,
        };

        self.permission_storage.save(permission, PrevSeq::Any).await
            .err().iter().for_each(|v| error!("{v}"));

        prev_permissions.push(permission);
        let json = serde_json::to_string(&prev_permissions).default_res()?;

        let path = format!("{}/{}", self.config.get_val(ConfigKey::RebacPath).await, obj_id.to_str_key());
        self.dht_map.put_with_seq(&path, &json, prev_seq).await.default_res()?;

        Ok(())
    }

    pub async fn put_permission(&self, current_user: u64, target_user: u64, obj_id: RebacKey,
                                permission_type: PermissionType) -> Result<(), String> {
        let required_user_permission = self.to_grant_mapping.get(&permission_type)
            .ok_or("permission cannot be granted")?;

        if !self.check_permission(current_user, obj_id, *required_user_permission).await? {
            return Err("access denied".to_string());
        }

        retry_fn(|| self.put_permission_impl(target_user, obj_id, permission_type))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(1), 30))
            .await
    }

    async fn delete_permission_impl(&self, target_user: u64, obj_id: RebacKey,
                                 permission_type: PermissionType) -> Result<(), String> {
        let user_permissions = self.get_permissions_of_all_users_without_rights_check(obj_id).await?;
        if let Some((mut exists_permission, dht_seq)) = user_permissions {

            let active_permissions_to_delete: Vec<_> = exists_permission.iter()
                .enumerate()
                .filter(|(i, v)| v.permission_type == permission_type && v.subject == target_user)
                .map(|(i, v)| (i, v.version))
                .collect();
            if !active_permissions_to_delete.is_empty() {
                let prev_seq = PrevSeq::Seq(Some(dht_seq));

                let permission = Permission {
                    subject: target_user,
                    object: obj_id,
                    permission_type,
                    version: active_permissions_to_delete.iter().max_by_key(|v| v.1).unwrap().1,
                    deleted: true,
                };

                self.permission_storage.save(permission, PrevSeq::Any).await
                    .err().iter().for_each(|v| error!("{v}"));

                active_permissions_to_delete.iter().rev().for_each(|(i, version)| { exists_permission.swap_remove(*i); });
                let json = serde_json::to_string(&exists_permission).default_res()?;

                let path = format!("{}/{}", self.config.get_val(ConfigKey::RebacPath).await, obj_id.to_str_key());
                self.dht_map.put_with_seq(&path, &json, prev_seq).await.default_res()?;
            }
        }

        Ok(())
    }


    pub async fn delete_permission(&self, current_user: u64, target_user: u64, obj_id: RebacKey,
                                permission_type: PermissionType) -> Result<(), String> {
        let required_user_permission = self.to_grant_mapping.get(&permission_type)
            .ok_or("permission cannot be revoked")?;

        if !self.check_permission(current_user, obj_id, *required_user_permission).await? {
            return Err("access denied".to_string());
        }

        retry_fn(|| self.delete_permission_impl(target_user, obj_id, permission_type))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(1), 30))
            .await
    }

    pub async fn put_permission_without_rights_check(&self, target_user: u64, obj_id: RebacKey,
                                permission_type: PermissionType) -> Result<(), String> {
        retry_fn(|| self.put_permission_impl(target_user, obj_id, permission_type))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(1), 30))
            .await
    }

    async fn get_permissions_of_all_users_without_rights_check(&self, obj_id: RebacKey) -> Result<Option<(Vec<Permission>, i64)>, String> {
        let path = format!("{}/{}", self.config.get_val(ConfigKey::RebacPath).await, obj_id.to_str_key());
        let permissions_json = self.dht_map.get(&path).await.default_res()?;

        if permissions_json.is_none() {
            return Ok(None)
        }
        let permissions_json = permissions_json.unwrap();

        let permissions = serde_json::from_str::<Vec<Permission>>(&permissions_json.0).default_res()?;
        let active_permissions = permissions.into_iter()
            .filter(|v| !v.deleted)
            .collect();
        Ok(Some((
            active_permissions,
            permissions_json.1
        )))
    }

    pub async fn get_permissions_of_all_users(&self, user_id: u64, obj_id: RebacKey) -> Result<Option<(Vec<Permission>, i64)>, UserPermissionsReadingError> {
        if self.check_permission(user_id, obj_id, PermissionType::ReadPermissionsOfOtherUsers).await? {
            if let Some((permissions, seq)) = self.get_permissions_of_all_users_without_rights_check(obj_id).await? {
                Ok(Some((permissions, seq)))
            } else {
                Ok(None)
            }
        } else {
            Err(UserPermissionsReadingError::AccessDenied)
        }
    }

    pub async fn get_user_permissions(&self, user_id: u64, obj_id: RebacKey) -> Result<Option<(Vec<Permission>, i64)>, String> {
        if let Some((permissions, seq)) = self.get_permissions_of_all_users_without_rights_check(obj_id).await? {
            let user_permissions = permissions.into_iter()
                .filter(|v| v.subject == user_id)
                .collect();

            Ok(Some((user_permissions, seq)))
        } else {
            Ok(None)
        }
    }

    pub async fn check_permission(&self, user: u64, rebac_key: RebacKey, target: PermissionType) -> Result<bool, String> {
        let user_permissions = self.get_user_permissions(user, rebac_key).await?
            .unwrap_or((vec![], 0));

        if user_permissions.0.iter().any(|permission| self.contains_permission(target, permission.permission_type)) {
            return Ok(true);
        }

        let dependencies = self.permissive_permissions.get(&target);
        if dependencies.is_none() {
            return Ok(false);
        }

        let futures: Vec<_> = dependencies.unwrap().iter().
            map(async |(get_other_ids_func, target_permission)| {
                let (other_uid, other) = get_other_ids_func.get(user, rebac_key).await?;

                let futures: Vec<_> = other.into_iter()
                    .map(|v| self.check_permission(other_uid, v, *target_permission))
                    .collect();

                Ok(join_all(futures).await.into_iter().any(|v| v.is_ok() && v.unwrap())) // просыпаем ошибки
        }).collect();

        let completed_futures: Vec<Result<bool, String>> = join_all(futures).await;
        if completed_futures.iter().any(|v| *v.as_ref().unwrap_or(&false)) {
            return Ok(true);
        } else {
            if completed_futures.iter().any(Result::is_err) {
                return Err(completed_futures.into_iter().filter(Result::is_err).map(|v| v.err().unwrap()).join(", "));
            }
        }
        Ok(false)
    }


    fn contains_permission(&self, target: PermissionType, root: PermissionType) -> bool {
        if target as isize == root as isize {
            return true;
        }

        for child in self.additional_permissions.get(&root).unwrap_or(&vec![]) {
            if self.contains_permission(target, *child) {
                return true
            }
        }

        false
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn check_contains_permission() {
        assert_eq!(0, 0);
    }
}

// const PERMISSION_CHILDS: &[(PermissionType, &[PermissionType])] = &[
//     (PermissionType::All, &[PermissionType::Read, PermissionType::Write, PermissionType::GrantRead, PermissionType::GrantWrite])
// ];

// const fn contains_permission(target: PermissionType, root: PermissionType) -> bool {
//     if target as isize == root as isize{
//         return true;
//     }
//
//     const_for!(permission_i in 0..PERMISSION_CHILDS.len() => {
//         let (parent, childs) = PERMISSION_CHILDS[permission_i];
//
//         if parent as isize == root as isize {
//             const_for!(child_i in 0..childs.len() => {
//                 let child = childs[child_i];
//
//                 if contains_permission(target, child) {
//                     return true
//                 }
//             })
//         }
//     });
//
//     false
// }