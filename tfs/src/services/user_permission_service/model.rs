use serde::{Deserialize, Serialize};
use crate::common::models::PermissionType;

#[derive(Serialize, Deserialize)]
pub struct PermissionE {
    pub login: String,
    pub permission_type: PermissionType
}