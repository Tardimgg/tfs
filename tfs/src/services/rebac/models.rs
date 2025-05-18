use serde::{Deserialize, Serialize};
use crate::common::models::{ObjType, PermissionType};

#[derive(PartialEq, Eq, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RebacKey {
    pub obj_type: ObjType,
    pub obj_id: u64
}

impl RebacKey {
    pub fn new(obj_type: ObjType, obj_id: u64) -> Self {
        Self { obj_type, obj_id }
    }

    pub fn to_str_key(&self) -> String {
        format!("{}-{}", self.obj_type.to_str_key(), self.obj_id)
    }
}

#[derive(Copy, Clone, Serialize, Deserialize, Debug)]
pub struct Permission {
    pub subject: u64,
    pub object: RebacKey,
    pub permission_type: PermissionType,
    pub version: u64,
    pub deleted: bool
}
