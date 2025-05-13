use std::sync::Arc;
use actix_web::{post, web, HttpRequest, HttpResponse, Responder, Scope};
use actix_web::web::Json;
use serde::Deserialize;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::common::exceptions::ApiException;
use crate::services::user_service::models::{NewUser, UserToken};
use crate::services::user_service::user_service::UserService;


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
async fn refresh_token(req: HttpRequest, user: AuthenticatedUser, user_service: web::Data<Arc<UserService>>) -> Result<Json<UserToken>, ApiException> {
    let refresh_token = req
        .headers()
        .get("refresh_token")
        .ok_or(ApiException::BadRequest("there is no refresh token".to_string()))?
        .to_str()
        .unwrap()
        .to_owned();

    let token = user_service.get_ref().refresh_token(user, &refresh_token).await?;
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


pub fn config() -> Scope {
    web::scope("user")
        .service(get_token)
        .service(refresh_token)
        .service(create_user)
}