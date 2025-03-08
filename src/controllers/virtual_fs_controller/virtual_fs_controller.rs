use std::error::Error;
use std::{iter, thread};
use std::fmt::format;
use std::io::{Read};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use actix_files::NamedFile;
use actix_multipart::form::{MultipartCollect, MultipartForm};
use actix_multipart::form::tempfile::TempFile;
use actix_multipart::Multipart;
use actix_web::{get, HttpRequest, HttpResponse, post, Responder, ResponseError, Scope, web};
use actix_web::error::HttpError;
use actix_web::http::header::{HeaderName, Range};
use actix_web::web::{BufMut, Bytes, Json};
use async_stream::stream;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, BufReader};
use tokio_util::io::ReaderStream;
use crate::common::buffered_consumer::BufferedConsumer;
use crate::controllers::virtual_fs_controller::error::ApiException;
use crate::services::file_storage::model::{FileMeta, FolderMeta, NodeType};
use crate::services::virtual_fs::virtual_fs::VirtualFS;

#[derive(Serialize, Deserialize)]
struct V {
    v: String
}

#[get("/folder{tail:.*}")]
async fn get_folder(folder_path: web::Path<String>, fs: web::Data<Arc<VirtualFS>>) -> Result<Json<FolderMeta>, ApiException> {
    Ok(web::Json(fs.get_ref().get_folder_content(&folder_path).await?))
}

#[post("/folder/{tail:.*}")]
async fn create_folder(folder_path: web::Path<String>, fs: web::Data<Arc<VirtualFS>>) -> HttpResponse {
    fs.get_ref().create_folder(&folder_path).await;
    HttpResponse::Ok().into()
}

#[get("/meta/node/{tail:.*}")]
async fn get_file_meta(file_path: web::Path<String>, fs: web::Data<Arc<VirtualFS>>) -> Result<impl Responder, ApiException> {
    let res = fs.get_ref().get_node_meta(&file_path).await?;
    Ok(web::Json(res))
}

#[get("/stored_parts/{tail:.*}")]
async fn get_stored_parts(filepath: web::Path<String>, req: HttpRequest, fs: web::Data<Arc<VirtualFS>>) -> Result<impl Responder, ApiException> {
    let meta = fs.get_ref().get_stored_parts_meta(&filepath).await.map_err(|v| ApiException::InternalError(v))?;
    Ok(web::Json(meta))

}

#[get("/file/{tail:.*}")]
async fn get_file(file_path: web::Path<String>, req: HttpRequest, fs: web::Data<Arc<VirtualFS>>) -> Result<impl Responder, ApiException> {

    let range_header_o = req.headers().get(actix_web::http::header::RANGE);
    let range_o = if let Some(range_header) = range_header_o {
        let range_string = range_header.to_str().map_err(|v| ApiException::BadRequest)?;
        Some(Range::from_str(range_string).map_err(|v| ApiException::BadRequest)?)
    } else {
        None
    };

    // let file = actix_files::NamedFile::open_async("/home/oop/RustroverProjects/tfs/root/asd/chunks/ALL_v0").await.unwrap();
    // return Ok(file)
    // кажется поддержки возврата несколких чанков нет даже в NamedFile => нужно реализовать свою структуру,
    // которая будет собирать чанки вместе и между ними писать нужную мету
    // https://github.com/actix/actix-web/pull/227
    // https://github.com/actix/actix-web/issues/60z`

    let mut file_o = fs.get_ref().get_file_content(&file_path, range_o).await?;
    if let Some(file) = file_o {
        return Ok(HttpResponse::Ok().streaming(file.get_stream()))
    }
    // HttpResponse::PartialContent().
    Ok(HttpResponse::NotFound().into())
}

#[post("/file/{tail:.*}")]
async fn create_file(filename: web::Path<String>, mut payload: web::Payload, fs: web::Data<Arc<VirtualFS>>) -> Result<Json<FileMeta>, ApiException> {
    let mut meta =  fs.get_ref().create_file(&filename, payload).await;
    Ok(web::Json(FileMeta::builder().name(filename.to_string()).node_type(NodeType::File).build()))
}

pub fn config() -> Scope {
    // let a = Actor
    web::scope("virtual_fs")
        .service(get_folder)
        .service(create_folder)
        .service(get_file)
        .service(get_stored_parts)
        .service(get_file_meta)
        .service(create_file)
}