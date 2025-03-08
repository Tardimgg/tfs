use std::error::Error;
use std::fmt::format;
use std::time::Duration;
use actix_web::http::header::Range;
use awc::Client;
use reqwest::StatusCode;
use tryhard::retry_fn;
use crate::client;
use crate::common::default_error::DefaultError;
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::common::retry_police::linear_backoff_with_jitter::LinearBackoffWithJitter;
use crate::services::dht_map::model::DhtNodeId;
use crate::services::file_storage::file_storage::FileStream;
use crate::services::virtual_fs::models::StoredFileRange;

#[derive(Default)]
pub struct InternalCommunication {
    client: reqwest::Client
}

impl InternalCommunication {

    pub async fn send_file(&self, filename: &str, node: DhtNodeId, file: FileStream, range: Range) -> Result<(), String> {
        // self.client.put(format!("http://{}:{}/virtual_fs/file/{}", node.ip, node.port, filename))
        //     .insert_header(range)
        //     .send_stream(file.get_stream())
        //     .await
        //     .unwrap();
        let stream = match file {
            FileStream::Payload(qwe) => { panic!()}
            FileStream::TokioFile(s) => s
        };

        self.client.post(format!("http://{}:{}/virtual_fs/file/{}", node.ip, node.port, filename))
            // .header(reqwest::range)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map(|v| ())
            .default_res()
    }

    pub async fn get_stored_parts_of_file(&self, filename: &str, node: DhtNodeId) -> Result<Vec<StoredFileRange>, String> { // видимо нужный какой то общий FileRange
        let response = retry_fn(|| {
            self.client.get(format!("http://{}:{}/virtual_fs/stored_parts/{}", node.ip, node.port, filename)).send()
        }).retries(3)
            .custom_backoff(LinearBackoffWithJitter::new(Duration::from_secs(3), 2, 30))
            .await;

        // println!("response {:?}", response.unwrap().text().await);


        Ok(response.map_err(|v| v.to_string())?
            .json::<Vec<StoredFileRange>>()
            .await
            .map_err(|v| v.to_string())?)




        // if res.status() == StatusCode::OK {
        //     Ok(res.into())
        // } else {
        //     Err(res.text().unwrap())
        // }
    }

}