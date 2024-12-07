mod error;

use std::error::Error;
use actix_web::{get, HttpResponse, post, Responder, Scope, web};
use actix_web::error::HttpError;
use actix_web::web::Json;
use serde::{Deserialize, Serialize};
use crate::controllers::virtual_fs_controller::error::ApiException;
use crate::services::virtual_fs::models::{FileMeta, FolderMeta};
use crate::services::virtual_fs::VirtualFS;

#[derive(Serialize, Deserialize)]
struct V {
    v: String
}


#[get("/folder/{id}")]
async fn get_folder(folder_path: web::Path<String>, fs: web::Data<VirtualFS>) -> Result<Json<FolderMeta>, ApiException> {
    Ok(web::Json(fs.get_ref().get_folder_content(&folder_path)))
}

#[post("/folder")]
async fn create_folder(folder_path: web::Path<String>, fs: web::Data<VirtualFS>) -> Result<Json<FileMeta>, ApiException> {
    Ok(web::Json(fs.get_ref().create_folder(&folder_path)))
}

#[get("/file/{id}")]
async fn get_file(folder_path: web::Path<String>, fs: web::Data<VirtualFS>) -> Result<Json<()>, ApiException> {
    Ok(web::Json(fs.get_ref().get_file_content(&folder_path)))
}

#[post("/file")]
async fn create_file(folder_path: web::Path<String>, fs: web::Data<VirtualFS>) -> Result<Json<FileMeta>, ApiException> {
    Ok(web::Json(fs.get_ref().create_file(&folder_path, "".to_string())))
}

pub fn config() -> Scope {
    // let a = Actor
    web::scope("virtual_fs")
        .service(get_folder)
        .service(create_folder)
        .service(get_file)
        .service(create_file)
}