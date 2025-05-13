use actix_web::{FromRequest, HttpRequest};
use actix_web::dev::Payload;

#[derive(Copy, Clone)]
pub struct AuthenticatedUser {
    pub uid: u64
}