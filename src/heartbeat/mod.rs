use std::cell::{Cell, RefCell};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use actix_web::http::header::{ByteRangeSpec, Range};
use dashmap::DashMap;
use rand::Rng;
use rand::rngs::ThreadRng;
use tokio::sync::mpsc::Receiver;
use typed_builder::TypedBuilder;
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::file_storage::file_storage::FileStorage;
use crate::services::internal_communication::InternalCommunication;
use crate::services::virtual_fs::VirtualFS;
// static dht: DistributedMap = 0;

#[derive(TypedBuilder)]
pub struct Heartbeat {
    fs: Arc<VirtualFS>,
    config: Arc<GlobalConfig>,
    storage: Arc<dyn FileStorage + Send + Sync>,

    #[builder(default=Arc::new(DashMap::new()))]
    state: Arc<DashMap<String, SystemTime>>
}

thread_local! {
    pub static CLIENT: RefCell<InternalCommunication> = RefCell::new(InternalCommunication::default());
    pub static RANDOM: RefCell<ThreadRng> = RefCell::new(rand::thread_rng());
}
impl Heartbeat {
    pub async fn init(&self, mut update_receiver: Receiver<StateUpdate>) {

        let map_ref = self.state.clone();
        let updater = tokio::spawn(async move {
            if let Some(update) = update_receiver.recv().await {
                match update {
                    StateUpdate::NewFile(path) => {
                        map_ref.insert(path, SystemTime::now())
                    }
                };
            }
        });

        self.init_db().await;

        let mut interval = tokio::time::interval(Duration::from_millis(10));


        loop {
            interval.tick().await;
            self.replication_files().await;
        }
        tokio::join!(updater);
    }


    async fn init_db(&self) {

    }
    async fn replication_files(&self) {
        let keepers_result = self.fs.get_all_keepers().await;


        if let Ok(keepers) = keepers_result {
            for path in self.state.iter() {
                if let Ok(meta) = self.fs.get_node_meta(path.key()).await {
                    if meta.keepers.len() < usize::from_str(&self.config.get_val(ConfigKey::ReplicationFactor).await).unwrap() {
                        let keeper_n = RANDOM.take().gen_range(0..keepers.len());
                        let keeper = keepers[keeper_n];

                        // temp temp temp^2 code
                        let mut file = self.storage.get_file(path.key(), None, None).await;
                        let mut file = self.storage.get_file(path.key(), None, None).await.unwrap();
                        CLIENT.take().send_file(keeper, file.swap_remove(0).1, Range::Bytes(vec![ByteRangeSpec::From(0)])).await;
                    }
                }
            }
        }
    }
}