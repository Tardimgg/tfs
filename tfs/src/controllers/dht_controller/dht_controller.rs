use std::sync::Arc;
use actix_web::{get, post, web, HttpResponse, Scope};
use actix_web::web::Json;
use serde::Deserialize;
use crate::common::default_error::DefaultError;
use crate::common::exceptions::ApiException;
use crate::controllers::virtual_fs_controller::virtual_fs_controller::{create_file, create_folder, get_file, get_file_meta, get_folder, get_stored_parts, save_existing_chunk};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::PrevSeq;

#[get("/{tail:.*}")]
async fn get_val(key: web::Path<String>, map: web::Data<Arc<DHTMap>>) -> Result<Json<String>, ApiException> {
    let ans = map.get_ref().get(&key).await.default_res()?.ok_or(ApiException::NotFound)?;
    Ok(web::Json(format!("{}; seq={}", ans.0, ans.1)))
}

#[derive(Deserialize)]
struct PutQuery {
    cas: Option<i64>
}

#[post("/{tail:.*}")]
async fn put_val(key: web::Path<String>, value: web::Payload, seq: web::Query<PutQuery>, map: web::Data<Arc<DHTMap>>) -> Result<HttpResponse, ApiException> {
    if let Some(cas) = seq.cas {
        map.get_ref().put_with_seq(&key, &String::from_utf8_lossy(value.to_bytes().await.unwrap().as_ref()), PrevSeq::Seq(Some(cas))).await.default_res()?;
    } else {
        map.get_ref().put(&key, &String::from_utf8_lossy(value.to_bytes().await.unwrap().as_ref())).await.default_res()?;
    }

    Ok(HttpResponse::Ok().into())
}


pub fn config() -> Scope {
    // let a = Actor
    web::scope("dht")
        .service(get_val)
        .service(put_val)
}