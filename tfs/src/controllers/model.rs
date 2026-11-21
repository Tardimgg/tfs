use std::fmt::{Debug, Display, Formatter};
use actix_web::ResponseError;
use actix_web::http::StatusCode;
use crate::common::exceptions::ApiException;

impl ResponseError for ApiException {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiException::FileNotFound => StatusCode::NOT_FOUND,
            ApiException::NotFound => StatusCode::NOT_FOUND,
            ApiException::FolderNotFound => StatusCode::NOT_FOUND,
            ApiException::FileAlreadyExist => StatusCode::CONFLICT,
            ApiException::FolderAlreadyExist => StatusCode::CONFLICT,
            ApiException::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiException::Conflict(_) => StatusCode::CONFLICT,
            ApiException::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiException::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
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
