mod error;
mod model;

use std::error::Error;
use std::{iter, thread};
use std::fmt::format;
use std::io::{Read};
use std::path::Path;
use actix_files::NamedFile;
use actix_multipart::form::{MultipartCollect, MultipartForm};
use actix_multipart::form::tempfile::TempFile;
use actix_multipart::Multipart;
use actix_web::{get, HttpResponse, post, Responder, ResponseError, Scope, web};
use actix_web::error::HttpError;
use actix_web::web::{BufMut, Bytes, Json};
use async_stream::stream;
use futures::{StreamExt, TryStreamExt};
use model::UploadFile;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, BufReader};
use tokio_util::io::ReaderStream;
use crate::common::buffered_consumer::BufferedConsumer;
use crate::controllers::virtual_fs_controller::error::ApiException;
use crate::services::virtual_fs::models::{FileMeta, FolderMeta};
use crate::services::virtual_fs::VirtualFS;

#[derive(Serialize, Deserialize)]
struct V {
    v: String
}

#[get("/folder{tail:.*}")]
async fn get_folder(folder_path: web::Path<String>, fs: web::Data<VirtualFS>) -> Result<Json<FolderMeta>, ApiException> {
    Ok(web::Json(fs.get_ref().get_folder_content(&folder_path).await?))
}

#[post("/folder/{tail:.*}")]
async fn create_folder(folder_path: web::Path<String>, fs: web::Data<VirtualFS>) -> HttpResponse {
    fs.get_ref().create_folder(&folder_path).await;
    HttpResponse::Ok().into()
}

#[get("/file/{tail:.*}")]
async fn get_file(file_path: web::Path<String>, fs: web::Data<VirtualFS>) -> Result<impl Responder, ApiException> {
    let mut file = fs.get_ref().get_file_content(&file_path).await?;
    Ok(HttpResponse::Ok().streaming(file.get_stream()))
}

#[post("/file/{tail:.*}")]
async fn create_file(filename: web::Path<String>, mut multipart: web::Payload, fs: web::Data<VirtualFS>) -> Result<Json<FileMeta>, ApiException> {
    let mut meta =  fs.get_ref().create_file(&filename, multipart).await;
    Ok(web::Json(FileMeta::builder().name(filename.to_string()).build()))
}

pub fn config() -> Scope {
    // let a = Actor
    web::scope("virtual_fs")
        .service(get_folder)
        .service(create_folder)
        .service(get_file)
        .service(create_file)
}