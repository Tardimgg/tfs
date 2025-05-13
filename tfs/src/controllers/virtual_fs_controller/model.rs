use actix_multipart::form::MultipartForm;
use actix_multipart::form::tempfile::TempFile;
use actix_web::{FromRequest, HttpRequest, HttpResponse};

#[derive(MultipartForm)]
pub struct UploadFile {
    #[multipart(limit = "100MB")]
    pub file: TempFile
}