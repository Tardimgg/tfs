use actix_web::http::header::Range;
use awc::Client;
use crate::client;
use crate::services::dht_map::model::DhtNodeId;
use crate::services::file_storage::file_storage::FileStream;

#[derive(Default)]
pub struct InternalCommunication {
    client: Client
}

impl InternalCommunication {

    pub async fn send_file(&self, node: DhtNodeId, file: FileStream, range: Range) {
        self.client.put(format!("{}:{}", node.ip, node.port))
            .insert_header(range)
            .send()
            .await
            .unwrap();

    }

}