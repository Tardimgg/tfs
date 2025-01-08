use actix_multipart::form::MultipartForm;
use actix_multipart::form::tempfile::TempFile;

#[derive(MultipartForm)]
pub struct UploadFile {
    #[multipart(limit = "100MB")]
    pub file: TempFile
}