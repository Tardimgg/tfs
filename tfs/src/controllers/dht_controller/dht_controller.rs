use std::sync::Arc;
use actix_web::{get, post, web, HttpResponse, Scope};
use actix_web::web::Json;
use serde::Deserialize;
use crate::common::default_error::DefaultError;
use crate::common::exceptions::ApiException;
use crate::controllers::virtual_fs_controller::virtual_fs_controller::{create_file, create_folder, get_file, get_file_meta, get_folder, get_stored_parts, save_existing_chunk};
use crate::services::dht_map::codec::DhtValue;
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::PrevSeq;
use crate::services::dtls_revocation_registry::REVOCATION_DHT_KEY;

#[get("/{tail:.*}")]
async fn get_val(key: web::Path<String>, map: web::Data<Arc<DHTMap>>) -> Result<Json<String>, ApiException> {
    map.check_security().map_err(|error| ApiException::ServiceUnavailable(error.to_string()))?;
    let result = map.get_ref().get_value(&key).await;
    map.check_security().map_err(|error| ApiException::ServiceUnavailable(error.to_string()))?;
    let (value, seq) = result.default_res()?.ok_or(ApiException::NotFound)?;
    let value = match value {
        DhtValue::Text(value) => value,
        DhtValue::UnsignedInteger(value) => value.to_string(),
        DhtValue::DhtNodeList(value) => serde_json::to_string(&value).default_res()?,
        DhtValue::GlobalFileInfo(value) => serde_json::to_string(&value).default_res()?,
        DhtValue::PermissionList(value) => serde_json::to_string(&value).default_res()?,
    };
    Ok(web::Json(format!("{value}; seq={seq}")))
}

#[derive(Deserialize)]
struct PutQuery {
    cas: Option<i64>
}

#[post("/{tail:.*}")]
async fn put_val(key: web::Path<String>, value: web::Payload, seq: web::Query<PutQuery>, map: web::Data<Arc<DHTMap>>) -> Result<HttpResponse, ApiException> {
    if key.as_str() == REVOCATION_DHT_KEY {
        return Err(ApiException::Forbidden("reserved DHT key must use the signed revocation API".to_string()));
    }
    map.check_security().map_err(|error| ApiException::ServiceUnavailable(error.to_string()))?;
    let value = String::from_utf8_lossy(value.to_bytes().await.unwrap().as_ref()).into_owned();
    let result = if let Some(cas) = seq.cas {
        map.get_ref().put_with_seq(&key, &value, PrevSeq::Seq(Some(cas))).await
    } else {
        map.get_ref().put(&key, &value).await
    };
    map.check_security().map_err(|error| ApiException::ServiceUnavailable(error.to_string()))?;
    result.default_res()?;

    Ok(HttpResponse::Ok().into())
}


pub fn config() -> Scope {
    // let a = Actor
    web::scope("dht")
        .service(get_val)
        .service(put_val)
}
