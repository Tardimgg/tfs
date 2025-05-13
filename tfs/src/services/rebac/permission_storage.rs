use async_trait::async_trait;
use crate::services::dht_map::model::PrevSeq;
use crate::services::rebac::models::Permission;

#[async_trait(?Send)]
pub trait PermissionStorage {
    async fn save(&self, permission: Permission, prev_seq: PrevSeq) -> Result<(), String>;
}