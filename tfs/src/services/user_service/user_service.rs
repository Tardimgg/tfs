use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Add;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use hmac::{Hmac, Mac};
use jwt::{SignWithKey, VerifyWithKey};
use rand::prelude::ThreadRng;
use rand::Rng;
use sha2::{Digest, Sha256};
use tryhard::retry_fn;
use typed_builder::TypedBuilder;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::common::common::{generate_token, read_stream_to_string};
use crate::common::default_error::DefaultError;
use crate::common::models::{JwtToken, ObjType, PermissionType, TokenAction, TokenType, UserInfo};
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::PrevSeq;
use crate::services::file_storage::file_storage::FileStream;
use crate::services::permission_service::permission_service::PermissionService;
use crate::services::user_service::models::{NewUser, UserToken};
use crate::services::virtual_fs::models::{FsNode, GlobalFileInfo};
use crate::services::virtual_fs::virtual_fs::VirtualFS;

#[derive(TypedBuilder)]
pub struct UserService {
    virtual_fs: Arc<VirtualFS>,
    permission_service: Arc<PermissionService>,
    config: Arc<GlobalConfig>
}

thread_local! {
    static RANDOM: RefCell<ThreadRng> = RefCell::new(rand::thread_rng());
}

impl UserService {

    pub async fn user_by_id(&self, uid: u64) -> Result<Option<UserInfo>, String> {
        let path = self.config.get_val(ConfigKey::UsersPath).await;
        // тут найдем только если юзер хранится локально, нужно переделать на нормальное скачивание
        // let user_info_stream = self.virtual_fs.get_file_content(&format!("{}/{}", path, uid), None)
        let user_info_stream = self.virtual_fs.get_node_stream(&format!("{}/{}", path, uid))
            .await
            .default_res()?;

        if user_info_stream.is_none() {
            return Ok(None);
        }
        let user_info_string = read_stream_to_string(user_info_stream.unwrap().get_stream()).await?;

        let fs_node = serde_json::from_str::<FsNode>(&user_info_string)
            .default_res()?;
        if let FsNode::UserInfo(user_info) = fs_node {
            Ok(Some(user_info))
        } else {
            Err("internal error".to_string())
        }

    }

    async fn verify_password(&self, uid: u64, password: &str) -> Result<bool, String> {
        let user = self.user_by_id(uid).await?
            .ok_or("user does not exist".to_string())?;

        let mut hasher = Sha256::new();
        hasher.update(password);
        let hashed_password = format!("{:x}", hasher.finalize());

        Ok(user.hashed_password == hashed_password)
    }

    pub async fn create_user_impl(&self, login: &str, password: &str) -> Result<NewUser, String> {
        if let Some(uid) = self.virtual_fs.uid_by_login(login).await? {
            return Err("user already exists".to_string())
        }

        let uid = RANDOM.take().gen_range(0..i32::MAX) as u64;

        let mut hasher = Sha256::new();
        hasher.update(password);
        let hashed_password = format!("{:x}", hasher.finalize());

        let user = UserInfo {
            uid,
            login: login.to_string(),
            hashed_password
        };
        let path = format!("{}/{}", self.config.get_val(ConfigKey::UsersPath).await, uid);

        // let stream = FileStream::StringData(Some(serde_json::to_string(&user).default_res()?));

        // self.virtual_fs.put_file(&path, stream, None).await.default_res()?;
        self.virtual_fs.save_node(&path, &FsNode::UserInfo(user), 0, PrevSeq::Seq(None)).await.default_res()?;
        // крч, юзеров хранить среди данных конечно прикольно, но это не очень удобно, да и
        // преимущества такого действа не очень понятны

        let user_token = self.generate_user_tokens(uid).await;
        Ok(NewUser {
            uid,
            token: user_token
        })
    }

    pub async fn create_user(&self, login: &str, password: &str) -> Result<NewUser, String> {
        let user = retry_fn(|| self.create_user_impl(login, password))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 30))
            .await?;

        self.init_user_folder(&user, login).await?;
        Ok(user)
    }

    async fn init_user_folder(&self, user: &NewUser, user_login: &str) -> Result<(), String>{

        let home = self.config.get_val(ConfigKey::HomePath).await;
        let user_folder = format!("{}/{}", home, user_login);
        let folder = self.virtual_fs.create_folder(&user_folder).await.default_res()?;
        self.permission_service.put_permission_without_rights_check(user.uid, ObjType::Folder, folder.id,
                                                                    PermissionType::All, None)
            .await
    }

    pub async fn get_token(&self, login: &str, password: &str) -> Result<UserToken, String> {
        let uid = self.virtual_fs.uid_by_login(&login).await?;
        if let None = uid {
            return Err("the user does not exist".to_string())
        }
        let uid = uid.unwrap();

        if self.verify_password(uid, password).await? {
            // добавить бан старого токена

            Ok(self.generate_user_tokens(uid).await)
        } else {
            Err("invalid token".to_string())
        }
    }

    pub async fn refresh_token(&self, user: AuthenticatedUser, refresh_token: &str) -> Result<UserToken, String> {
        let refresh_token_uid = self.validate_token(refresh_token, TokenAction::Refresh).await?;
        if refresh_token_uid != user.uid {
            return Err("invalid refresh token".to_string());
        }

        Ok(self.generate_user_tokens(refresh_token_uid).await)
    }

    pub async fn revoke_token(&self, token: &str) -> Result<(), String> {
        Err("todo".to_string())
    }

    async fn validate_token(&self, token: &str, token_action: TokenAction) -> Result<u64, String> {
        let secret = self.config.get_val(ConfigKey::SecretSigningKey).await;
        let hmac: Hmac<Sha256> = Hmac::new_from_slice(secret.as_bytes()).unwrap();

        let jwt_token: JwtToken = token.verify_with_key(&hmac).default_res()?;
        if jwt_token.token_action as isize != token_action as isize {
            return Err("token has an incorrect type".to_string())
        }
        // if !claims.contains_key("uid") || !claims.contains_key("expires") {
        //     return Err("invalid uid".to_string());
        // }
        // let uid_str = claims.get("uid").unwrap();
        // let expires_str = claims.get("expires").unwrap();

        // let uid = u64::from_str(uid_str).default_res()?;
        // let expires = u64::from_str(expires_str).default_res()?;

        let now = SystemTime::now();

        if now.duration_since(UNIX_EPOCH).unwrap().as_secs() > jwt_token.expires {
            return Err("token is expired".to_string())
        }
        Ok(jwt_token.uid)
    }

    async fn generate_user_tokens(&self, uid: u64) -> UserToken {
        let now = SystemTime::now();

        let access_token_expires = now.clone().add(Duration::from_secs(60 * 60 * 24 * 7))
            .duration_since(UNIX_EPOCH).unwrap().as_secs();
        let refresh_token_expires = now.clone().add(Duration::from_secs(60 * 60 * 24 * 30))
            .duration_since(UNIX_EPOCH).unwrap().as_secs();

        let secret = self.config.get_val(ConfigKey::SecretSigningKey).await;
        let access_token = generate_token(uid, access_token_expires, TokenAction::Access, secret.as_bytes());
        let refresh_token = generate_token(uid, refresh_token_expires, TokenAction::Refresh, secret.as_bytes());
        UserToken {
            access_token,
            refresh_token,
            token_type: TokenType::Bearer,
        }

    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_generate_user_token() {
        assert_eq!(0, 0);
    }
}