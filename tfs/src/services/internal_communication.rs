use std::error::Error;
use std::fmt::format;
use std::ops::Add;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use actix_web::http::header::Range;
use awc::Client;
use bytes::Bytes;
use futures::Stream;
use reqwest::header::{AUTHORIZATION, RANGE};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tryhard::retry_fn;
use typed_builder::TypedBuilder;
use crate::client;
use crate::common::common::generate_token;
use crate::common::default_error::DefaultError;
use crate::common::models::TokenAction;
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::common::retry_police::linear_backoff_with_jitter::LinearBackoffWithJitter;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::model::DhtNodeId;
use crate::services::file_storage::file_storage::FileStream;
use crate::services::virtual_fs::models::{FsNodeType, StoredFile};

#[derive(TypedBuilder)]
pub struct InternalCommunication {
    #[builder(default=reqwest::Client::default())]
    client: reqwest::Client,
    config: Arc<GlobalConfig>
}

#[derive(Debug, Deserialize, Serialize)]
struct SaveExistingRequest {
    version: u64,
    fs_node_type: FsNodeType
}

impl InternalCommunication {

    pub async fn send_file(&self, filename: &str, node: DhtNodeId, file: FileStream, range: Range, fs_node_type: FsNodeType, version: u64) -> Result<(), String> {
        // self.client.put(format!("http://{}:{}/virtual_fs/file/{}", node.ip, node.port, filename))
        //     .insert_header(range)
        //     .send_stream(file.get_stream())
        //     .await
        //     .unwrap();
        let stream = match file {
            FileStream::Payload(qwe) => { panic!()}
            FileStream::TokioFile(s) => s,
            FileStream::StringData(_) => { panic!() }
            // FileStream::Stream(s) => s
            FileStream::ReceiverStream(_) => { panic!() }
        };

        // let response = self.client.put(format!("http://{}:{}/virtual_fs/file/{}", node.ip, node.port, filename))
        let response = self.client.post(format!("http://{}:{}/virtual_fs/file/{}", node.ip, node.port, filename.trim_start_matches("/")))
            .header(RANGE, range.to_string())
            .header(AUTHORIZATION, format!("Bearer {}", self.internal_token().await))
            .body(reqwest::Body::wrap_stream(stream))
            .query(&SaveExistingRequest { version, fs_node_type })
            .send()
            .await
            .default_res()?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("status = {}. text = {}", response.status(), response.text().await.unwrap_or("".to_string())))
        }
    }

    pub async fn get_stored_versions_of_file(&self, filename: &str, node: DhtNodeId) -> Result<Vec<StoredFile>, String> { // видимо нужный какой то общий FileRange
        let response = retry_fn(async || {
            self.client.get(format!("http://{}:{}/virtual_fs/stored_parts/{}", node.ip, node.port, filename))
                .header(AUTHORIZATION, format!("Bearer {}", self.internal_token().await))
                .send()
                .await
        }).retries(3)
            .custom_backoff(LinearBackoffWithJitter::new(Duration::from_secs(3), 2, 30))
            .await;

        let response = response.map_err(|v| v.to_string())?;
        if response.status().is_success() {
            Ok(response
                .json::<Vec<StoredFile>>()
                .await
                .map_err(|v| v.to_string())?)
        } else {
            Err(format!("status = {}; text = {:?}", response.status(), response.text().await))
        }
    }

    pub async fn get_file_content(&self, filename: &str, node: DhtNodeId) -> Result<impl Stream<Item=Result<Bytes, reqwest::Error>>, String> {
        let response = retry_fn(|| async {
            self.client.get(format!("http://{}:{}/virtual_fs/file/{}", node.ip, node.port, filename))
                .header(AUTHORIZATION, format!("Bearer {}", self.internal_token().await))
                .send()
                .await
        }).retries(3)
            .custom_backoff(LinearBackoffWithJitter::new(Duration::from_secs(3), 2, 30))
            .await;

        let response = response.map_err(|v| v.to_string())?;
        if response.status().is_success() {
            Ok(response.bytes_stream())
        } else {
            Err(format!("status = {}; text = {:?}", response.status(), response.text().await))
        }
    }

    async fn internal_token(&self) -> String {

        let now = SystemTime::now();
        let expires = now.add(Duration::from_secs(60 * 5))
            .duration_since(UNIX_EPOCH).unwrap().as_secs();

        let self_uid = self.config.get_val(ConfigKey::SelfUid).await;
        let secret = self.config.get_val(ConfigKey::SecretSigningKey).await;
        generate_token(u64::from_str(&self_uid).unwrap(), expires, TokenAction::Access, secret.as_bytes())
    }
}