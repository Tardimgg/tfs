use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;
use typed_builder::TypedBuilder;
use crate::services::file_storage::model::FileMeta;

#[derive(Eq, PartialEq, Copy, Clone, Debug, Serialize, Deserialize)]
pub enum ObjType {
    #[serde(alias="file")]
    File,
    #[serde(alias="folder")]
    Folder
}

impl ObjType {
    pub fn to_str_key(&self) -> &str {
        match self {
            ObjType::File => "file",
            ObjType::Folder => "folder"
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize, Hash, EnumIter)]
pub enum PermissionType {
    #[serde(alias="read")]
    Read,
    #[serde(alias="write")]
    Write,
    #[serde(alias="read_recursively")]
    ReadRecursively,
    #[serde(alias="write_recursively")]
    WriteRecursively,
    #[serde(alias="grant_read")]
    GrantRead,
    #[serde(alias="grant_write")]
    GrantWrite,
    #[serde(alias="grant_read_recursively")]
    GrantReadRecursively,
    #[serde(alias="grant_write_recursively")]
    GrantWriteRecursively,
    #[serde(alias="read_permissions_of_other_users")]
    ReadPermissionsOfOtherUsers,
    #[serde(alias="read_permissions_of_other_users_recursively")]
    ReadPermissionsOfOtherUsersRecursively,
    #[serde(alias="all")]
    All
}

impl PermissionType {
    pub fn to_str_key(&self) -> &str {
        match self {
            PermissionType::Read => "read",
            PermissionType::Write => "write",
            PermissionType::ReadRecursively => "read_r",
            PermissionType::WriteRecursively => "write_r",
            PermissionType::GrantRead => "grant_read",
            PermissionType::GrantWrite => "grant_write",
            PermissionType::GrantReadRecursively => "grant_read_r",
            PermissionType::GrantWriteRecursively => "grant_write_r",
            PermissionType::All => "all",
            PermissionType::ReadPermissionsOfOtherUsers => "read_perm_oth_usr",
            PermissionType::ReadPermissionsOfOtherUsersRecursively => "read_perm_oth_usr_r"
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct UserInfo {
    pub uid: u64,
    pub login: String,
    pub hashed_password: String
}

#[derive(Serialize, Deserialize)]
pub struct JwtToken {
    pub expires: u64,
    pub uid: u64,
    pub token_action: TokenAction
}
#[derive(Serialize, Deserialize)]
pub enum TokenAction {
    Access, Refresh
}

#[derive(Serialize, Deserialize)]
pub enum TokenType {
    Bearer
}


#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct FolderContent {
    pub name: String,
    pub files: Vec<FileMeta>,
    pub id: u64
}

#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct FolderContentE {
    pub name: String,
    pub files: Vec<FileMeta>,
    pub is_partial: bool,
    pub id: u64
}