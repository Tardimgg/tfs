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
use actix_web::{get, HttpRequest, HttpResponse, post, Responder, ResponseError, Scope, web, put, patch, delete};
use actix_web::error::HttpError;
use actix_web::http::header::{ContentType, HeaderName, Range};
use actix_web::web::{BufMut, Bytes, Json};
use async_stream::stream;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, BufReader};
use tokio_util::io::ReaderStream;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::common::buffered_consumer::BufferedConsumer;
use crate::common::exceptions::ApiException;
use crate::common::models::{FolderContent, FolderContentE};
use crate::services::file_storage::file_storage::FileStream;
use crate::services::file_storage::model::{FileMeta, FolderMeta, NodeType};
use crate::services::shared_fs::shared_fs::SharedFS;
use crate::services::virtual_fs::models::{File, FsNodeType, GlobalFileInfo};
use crate::services::virtual_fs::virtual_fs::VirtualFS;

#[derive(Serialize, Deserialize)]
struct V {
    v: String
}

#[get("/folder/{tail:.*}")]
async fn get_folder(user: AuthenticatedUser, folder_path: web::Path<String>, fs: web::Data<Arc<SharedFS>>) -> Result<Json<FolderContentE>, ApiException> {
    Ok(web::Json(fs.get_ref().get_folder_content(user, &folder_path).await?))
}

#[post("/folder/{tail:.*}")]
async fn create_folder(user: AuthenticatedUser, folder_path: web::Path<String>, fs: web::Data<Arc<SharedFS>>) -> Result<HttpResponse, ApiException> {
    fs.get_ref().create_folder(user, &folder_path).await?;
    Ok(HttpResponse::Ok().into())
}

#[get("/meta/node/{tail:.*}")]
async fn get_node_meta(user: AuthenticatedUser, file_path: web::Path<String>, fs: web::Data<Arc<SharedFS>>) -> Result<Json<GlobalFileInfo>, ApiException> {
    let res = fs.get_ref().get_node_meta(user, &file_path).await?;
    Ok(web::Json(res))
}

#[get("/stored_parts/{tail:.*}")]
async fn get_stored_parts(user: AuthenticatedUser, filepath: web::Path<String>, req: HttpRequest, fs: web::Data<Arc<SharedFS>>) -> Result<impl Responder, ApiException> {
    let meta = fs.get_ref().get_stored_parts_meta(user, &filepath).await.map_err(|v| ApiException::InternalError(v))?;
    Ok(web::Json(meta))
}

#[get("/file_meta/{tail:.*}")]
async fn get_file_meta(user: AuthenticatedUser, file_path: web::Path<String>,
                          req: HttpRequest, fs: web::Data<Arc<SharedFS>>) -> Result<Json<File>, ApiException> {
    let mut file_o = fs.get_ref().get_file_meta(user, &file_path).await?;
    if let Some(file) = file_o {
        Ok(web::Json(file))
    } else {
        Err(ApiException::NotFound)
    }

}

#[get("/file/{tail:.*}")]
async fn get_file(user: AuthenticatedUser, file_path: web::Path<String>,
                  req: HttpRequest, fs: web::Data<Arc<SharedFS>>) -> Result<impl Responder, ApiException> {

    let range_header_o = req.headers().get(actix_web::http::header::RANGE);
    let range_o = parse_range_header(&req)?;

    // let file = actix_files::NamedFile::open_async("/home/oop/RustroverProjects/tfs/root/asd/chunks/ALL_v0").await.unwrap();
    // return Ok(file)
    // кажется поддержки возврата несколких чанков нет даже в NamedFile => нужно реализовать свою структуру,
    // которая будет собирать чанки вместе и между ними писать нужную мету
    // https://github.com/actix/actix-web/pull/227
    // https://github.com/actix/actix-web/issues/60

    let mut file_o = fs.get_ref().get_file_content(user, &file_path, range_o).await?;
    if let Some(file) = file_o {
        // тут вообще не обязательно возвращается json
        return Ok(HttpResponse::Ok().content_type(ContentType::json()).streaming(file.get_stream()))
    }
    // HttpResponse::PartialContent().
    Ok(HttpResponse::NotFound().into())
}

#[put("/file/{tail:.*}")]
async fn create_file(user: AuthenticatedUser, filename: web::Path<String>, payload: web::Payload, req: HttpRequest, fs: web::Data<Arc<SharedFS>>) -> Result<Json<FileMeta>, ApiException> {
    let meta =  fs.get_ref().put_user_file(user, &filename, FileStream::Payload(payload), None).await?;
    Ok(web::Json(meta))
}

#[delete("/file/{tail:.*}")]
async fn delete_file(user: AuthenticatedUser, filename: web::Path<String>, req: HttpRequest, fs: web::Data<Arc<SharedFS>>) -> Result<Json<()>, ApiException> {
    let meta = fs.get_ref().delete_user_file(user, &filename).await?;
    Ok(web::Json(meta))
}

#[patch("/file/{tail:.*}")]
async fn patch_file(user: AuthenticatedUser, filename: web::Path<String>, payload: web::Payload, req: HttpRequest, fs: web::Data<Arc<SharedFS>>) -> Result<Json<FileMeta>, ApiException> {
    let range = parse_range_header(&req)?;

    let meta =  fs.get_ref().put_user_file(user, &filename, FileStream::Payload(payload), range).await?;
    Ok(web::Json(meta))
}

#[derive(Debug, Deserialize)]
pub struct SaveExistingRequest {
    version: u64,
    fs_node_type: FsNodeType
}

#[post("/file/{tail:.*}")]
async fn save_existing_chunk(user: AuthenticatedUser, filename: web::Path<String>, payload: web::Payload, req: HttpRequest,
                             query: web::Query<SaveExistingRequest>,
                             fs: web::Data<Arc<SharedFS>>) -> Result<Json<FileMeta>, ApiException> {
    let range = parse_range_header(&req)?;

    let meta =  fs.get_ref().save_existing_chunk(user, &filename, payload, range,
                                                 query.fs_node_type, query.version).await?;
    Ok(web::Json(meta))
}

fn parse_range_header(req: &HttpRequest) -> Result<Option<Range>, ApiException> {
    let range_header_o = req.headers().get(actix_web::http::header::RANGE);
    if let Some(range_header) = range_header_o {
        let range_string = range_header.to_str().map_err(|v| ApiException::BadRequest("invalid range".to_string()))?;
        Ok(Some(Range::from_str(range_string).map_err(|v| ApiException::BadRequest("invalid range".to_string()))?))
    } else {
        Ok(None)
    }

}

pub fn config() -> Scope {
    // let a = Actor
    web::scope("virtual_fs")
        .service(get_folder)
        .service(create_folder)
        .service(get_file)
        .service(get_file_meta)
        .service(delete_file)
        .service(get_stored_parts)
        .service(get_node_meta)
        .service(create_file)
        .service(save_existing_chunk)
}