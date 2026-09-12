use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use actix_web::{dev::Payload, error::{ErrorServiceUnavailable, ErrorUnauthorized}, http::header::AUTHORIZATION, web, FromRequest, HttpRequest};
use actix_web_httpauth::extractors::bearer::BearerAuth;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use jwt::VerifyWithKey;
use serde::Deserialize;
use sha2::Sha256;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::common::models::{JwtToken, Role, TokenAction};
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::user_service::user_service::RoleAuthorizer;

mod ca;
pub(crate) use ca::CaAccessTokenVerifier;

fn single_bearer(req: &HttpRequest, payload: &mut Payload) -> Result<BearerAuth, actix_web::Error> {
    if req.headers().get_all(AUTHORIZATION).nth(1).is_some() {
        return Err(ErrorUnauthorized("multiple Authorization headers"));
    }
    BearerAuth::from_request(req, payload).into_inner().map_err(Into::into)
}

#[derive(Deserialize)]
struct AccessHeader {
    alg: AccessAlgorithm,
    #[serde(default, deserialize_with = "critical_header_present")]
    crit: bool,
}

fn critical_header_present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

#[derive(Deserialize)]
enum AccessAlgorithm {
    HS256,
    PS256,
}

fn access_header(token: &str) -> Result<AccessHeader, actix_web::Error> {
    let (header, _) = token.split_once('.').ok_or_else(|| ErrorUnauthorized("invalid access token"))?;
    let header = URL_SAFE_NO_PAD.decode(header).map_err(|_| ErrorUnauthorized("invalid access token"))?;
    if !header.trim_ascii_start().starts_with(b"{") {
        return Err(ErrorUnauthorized("invalid access token"));
    }
    serde_json::from_slice(&header).map_err(|_| ErrorUnauthorized("invalid access token"))
}

fn unix_now() -> Result<u64, actix_web::Error> {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ErrorServiceUnavailable("authentication clock unavailable"))
}

async fn verify_session(token: &str, config: Option<web::Data<Arc<GlobalConfig>>>) -> Result<SessionUser, actix_web::Error> {
    let config = config.ok_or_else(|| ErrorServiceUnavailable("authentication unavailable"))?;
    let secret = config.signing_key().await.map_err(ErrorServiceUnavailable)?;
    let hmac: Hmac<Sha256> = Hmac::new_from_slice(secret.as_bytes())
        .map_err(|_| ErrorServiceUnavailable("authentication unavailable"))?;
    let token: JwtToken = token.verify_with_key(&hmac)
        .map_err(|_| ErrorUnauthorized("invalid access token"))?;
    match token.token_action {
        TokenAction::Access => {},
        TokenAction::Refresh => return Err(ErrorUnauthorized("token has an incorrect type")),
    }
    if unix_now()? > token.expires {
        return Err(ErrorUnauthorized("token is expired"));
    }
    Ok(SessionUser { uid: token.uid })
}

pub(crate) struct RequiredUser {
    pub uid: u64,
    additional_roles: Vec<Role>,
}

impl FromRequest for RequiredUser {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let bearer = single_bearer(req, payload);
        let config = req.app_data::<web::Data<Arc<GlobalConfig>>>().cloned();
        let ca = req.app_data::<web::Data<CaAccessTokenVerifier>>().cloned();
        Box::pin(async move {
            let bearer = bearer?;
            let token = bearer.token();
            let header = access_header(token)?;
            match header.alg {
                AccessAlgorithm::HS256 => {
                    let session = verify_session(token, config).await?;
                    Ok(Self { uid: session.uid, additional_roles: Vec::new() })
                }
                AccessAlgorithm::PS256 => {
                    if header.crit {
                        return Err(ErrorUnauthorized("unsupported critical token header"));
                    }
                    ca.ok_or_else(|| ErrorServiceUnavailable("CA authentication unavailable"))?.verify(token)
                }
            }
        })
    }
}

pub(crate) struct SessionUser {
    pub uid: u64,
}

impl FromRequest for SessionUser {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let bearer = single_bearer(req, payload);
        let config = req.app_data::<web::Data<Arc<GlobalConfig>>>().cloned();
        Box::pin(async move {
            let bearer = bearer?;
            let token = bearer.token();
            match access_header(token)?.alg {
                AccessAlgorithm::HS256 => verify_session(token, config).await,
                AccessAlgorithm::PS256 => Err(ErrorUnauthorized("session access token required")),
            }
        })
    }
}

impl FromRequest for AuthenticatedUser {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        if req.headers().contains_key(AUTHORIZATION) {
            let user = RequiredUser::from_request(req, payload);
            return Box::pin(async move { Ok(Self { uid: user.await?.uid }) });
        }
        let config = req.app_data::<web::Data<Arc<GlobalConfig>>>().cloned();
        Box::pin(async move {
            let config = config.ok_or_else(|| actix_web::error::ErrorServiceUnavailable("authentication unavailable"))?;
            let uid = config.get_val(ConfigKey::AnonymousUid).await.parse::<u64>()
                .map_err(|_| actix_web::error::ErrorServiceUnavailable("anonymous user configuration invalid"))?;
            Ok(Self { uid })
        })
    }
}

pub(crate) trait RequiredRole {
    const ROLE: Role;
}

pub(crate) struct RawDhtAccess;
pub(crate) struct RevocationRegistryPublisher;
pub(crate) struct ManageRoles;

impl RequiredRole for RawDhtAccess {
    const ROLE: Role = Role::RawDhtAccess;
}

impl RequiredRole for RevocationRegistryPublisher {
    const ROLE: Role = Role::RevocationRegistryPublisher;
}

impl RequiredRole for ManageRoles {
    const ROLE: Role = Role::ManageRoles;
}

pub(crate) struct RoleAccess<Required: RequiredRole>(PhantomData<Required>);

impl<Required: RequiredRole + 'static> FromRequest for RoleAccess<Required> {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let user = RequiredUser::from_request(req, payload);
        let users = req.app_data::<web::Data<Arc<dyn RoleAuthorizer>>>().cloned();
        Box::pin(async move {
            let user = user.await?;
            if user.additional_roles.contains(&Required::ROLE) {
                return Ok(Self(PhantomData));
            }
            let users = users.ok_or_else(|| actix_web::error::ErrorServiceUnavailable("users unavailable"))?;
            match users.has_role(user.uid, Required::ROLE).await {
                Ok(true) => Ok(Self(PhantomData)),
                Ok(false) => Err(actix_web::error::ErrorForbidden("role required")),
                Err(_) => Err(actix_web::error::ErrorServiceUnavailable("role lookup unavailable")),
            }
        })
    }
}
