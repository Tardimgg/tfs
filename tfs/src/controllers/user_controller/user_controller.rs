use std::sync::Arc;
use actix_web::{post, web, HttpRequest, HttpResponse, Responder, Scope};
use actix_web::web::Json;
use serde::Deserialize;
use crate::common::exceptions::ApiException;
use crate::common::models::Role;
use crate::controllers::auth::{ManageRoles, RoleAccess, SessionUser};
use crate::services::dht_map::dht_map::StrictDhtError;
use crate::services::file_storage::errors::FileReadingError;
use crate::services::user_service::models::{NewUser, UserToken};
use crate::services::user_service::user_service::{UserAuthError, UserRoleGrantError, UserService};
use crate::services::virtual_fs::virtual_fs::RoleGrantError;

impl From<UserAuthError> for ApiException {
    fn from(error: UserAuthError) -> Self {
        match error {
            UserAuthError::Unavailable(error) => Self::ServiceUnavailable(error.to_string()),
            UserAuthError::Operation(error) => error.into(),
        }
    }
}

#[derive(Deserialize)]
struct AuthRequest {
    login: String,
    password: String
}

#[post("auth")]
async fn get_token(json: web::Json<AuthRequest>, user_service: web::Data<Arc<UserService>>) -> Result<Json<UserToken>, ApiException> {
    let token = user_service.get_ref().get_token(&json.login, &json.password).await?;

    Ok(web::Json(token))
}


#[post("refresh_token")]
async fn refresh_token(req: HttpRequest, user: SessionUser, user_service: web::Data<Arc<UserService>>) -> Result<Json<UserToken>, ApiException> {
    let refresh_token = req
        .headers()
        .get("refresh_token")
        .ok_or(ApiException::BadRequest("there is no refresh token".to_string()))?
        .to_str()
        .map_err(|_| ApiException::BadRequest("invalid refresh token header".to_string()))?;

    let token = user_service.get_ref().refresh_token(user.uid, refresh_token).await?;
    Ok(web::Json(token))
}

#[derive(Deserialize)]
struct RegistrationRequest {
    login: String,
    password: String
}

#[post("")]
async fn create_user(json: web::Json<RegistrationRequest>, user_service: web::Data<Arc<UserService>>) -> Result<Json<NewUser>, ApiException> {
    let user_info = user_service.get_ref().create_user(&json.login, &json.password).await?;

    Ok(Json(user_info))
}

impl From<UserRoleGrantError> for ApiException {
    fn from(error: UserRoleGrantError) -> Self {
        use RoleGrantError::*;
        match error {
            UserRoleGrantError::Grant(MissingUser) => Self::NotFound,
            UserRoleGrantError::Grant(IdentityMismatch | InvalidMetadata | Dht(StrictDhtError::Changed | StrictDhtError::Conflict(_))) => {
                Self::Conflict(error.to_string())
            }
            UserRoleGrantError::Grant(
                Dht(_) | BlobRead(_) | LocalVersionUnavailable(_) | Unconfirmed | Notification(_)
                | Read(FileReadingError::NotExist | FileReadingError::ChunkIsNotExist(_) | FileReadingError::Retryable(_)),
            )
            | UserRoleGrantError::LoginLookup(_) => Self::ServiceUnavailable(error.to_string()),
            UserRoleGrantError::Grant(Read(_) | Save(_) | Json(_) | VersionOverflow) => {
                Self::InternalError(error.to_string())
            }
        }
    }
}

#[derive(Deserialize)]
struct GrantRolesRequest {
    roles: Vec<Role>,
}

#[post("/{uid}/roles")]
async fn grant_roles(
    _access: RoleAccess<ManageRoles>,
    uid: web::Path<u64>,
    json: web::Json<GrantRolesRequest>,
    user_service: web::Data<Arc<UserService>>,
) -> Result<HttpResponse, ApiException> {
    if json.roles.is_empty() {
        return Err(ApiException::BadRequest("at least one role is required".to_string()));
    }
    user_service.grant_roles(uid.into_inner(), &json.roles).await?;
    Ok(HttpResponse::NoContent().finish())
}


pub fn config() -> Scope {
    web::scope("user")
        .service(get_token)
        .service(refresh_token)
        .service(create_user)
        .service(grant_roles)
}
