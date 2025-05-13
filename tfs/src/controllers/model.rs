use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use actix_web::{web, FromRequest, HttpRequest, ResponseError};
use actix_web::dev::Payload;
use actix_web::http::StatusCode;
use hmac::{Hmac, Mac};
use jwt::VerifyWithKey;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::common::exceptions::ApiException;
use crate::common::models::{JwtToken, TokenAction};
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::permission_service::permission_service::PermissionService;

impl FromRequest for AuthenticatedUser {
    type Error = actix_web::Error;
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let auth_header = req
            .headers()
            .get("Authorization")
            .map(|v| v.to_str().unwrap().to_owned());

        // let permission_service = req.app_data::<web::Data<Arc<PermissionService>>>();
        let config = req.app_data::<web::Data<Arc<GlobalConfig>>>().unwrap().clone();
        Box::pin(async move {

            if let None = auth_header {
                let anonymous_uid = u64::from_str(&config.get_val(ConfigKey::AnonymousUid).await).unwrap();
                return Ok(AuthenticatedUser {
                    uid: anonymous_uid
                });
            }
            let auth_token = auth_header.unwrap();

            let secret = config.get_val(ConfigKey::SecretSigningKey).await;
            let hmac: Hmac<Sha256> = Hmac::new_from_slice(secret.as_bytes()).unwrap();
            let json: JwtToken = auth_token
                .trim_start_matches("Bearer")
                .trim()
                .verify_with_key(&hmac)
                .map_err(|err| actix_web::error::ErrorUnauthorized(err.to_string()))?;

            if json.token_action as isize != TokenAction::Access as isize {
                return Err(actix_web::error::ErrorUnauthorized("token has an incorrect type"))
            }

            // let user_id_str = json.get("user_id")
            //     .ok_or(actix_web::error::ErrorUnauthorized("jwt does not contain user_id"))?;
            // let user_id = u64::from_str(user_id_str)
            //     .map_err(|err| actix_web::error::ErrorUnauthorized(format!("when parse user_id: {}", err.to_string())))?;

            let now = SystemTime::now();

            if now.duration_since(UNIX_EPOCH).unwrap().as_secs() > json.expires {
                return Err(actix_web::error::ErrorUnauthorized("token is expired"))
            }

            Ok(AuthenticatedUser {
                uid: json.uid
            })
            // if permission_service.clone().check_permission(user_id, 1).await.unwrap_or(false) {
            //     return Ok(AuthorizedUser {
            //         uid: user_id
            //     })
            // }
            // Err(actix_web::error::ErrorForbidden("access denied"))
        })
    }
}

impl ResponseError for ApiException {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiException::FileNotFound => StatusCode::NOT_FOUND,
            ApiException::NotFound => StatusCode::NOT_FOUND,
            ApiException::FolderNotFound => StatusCode::NOT_FOUND,
            ApiException::FileAlreadyExist => StatusCode::CONFLICT,
            ApiException::FolderAlreadyExist => StatusCode::CONFLICT,
            ApiException::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiException::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiException::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiException::UnknownNode(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl Display for ApiException {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self, f)
    }
}
