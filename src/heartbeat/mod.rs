mod models;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use actix_web::http::header::{ByteRangeSpec, Range};
use dashmap::DashMap;
use rand::Rng;
use rand::rngs::ThreadRng;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use typed_builder::TypedBuilder;
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::heartbeat::models::RangeEvent;
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::file_storage::file_storage::FileStorage;
use crate::services::file_storage::model::EndOfFileRange;
use crate::services::internal_communication::InternalCommunication;
use crate::services::virtual_fs::VirtualFS;
// static dht: DistributedMap = 0;

#[derive(TypedBuilder)]
pub struct Heartbeat {
    fs: Arc<VirtualFS>,
    config: Arc<GlobalConfig>,
    storage: Arc<dyn FileStorage + Send + Sync>,

    #[builder(default=Arc::new(DashMap::new()))]
    state: Arc<DashMap<String, SystemTime>>,
    cancellation_token: CancellationToken
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
            if self.cancellation_token.is_cancelled() {
                updater.abort();
                break;
            }
        }
        tokio::join!(updater);
    }


    async fn init_db(&self) {

    }
    async fn replication_files(&self) {
        let keepers_result = self.fs.get_all_keepers().await;

        let replication_factor = usize::from_str(&self.config.get_val(ConfigKey::ReplicationFactor).await).unwrap();
        if let Ok(keepers) = keepers_result {
            for path in self.state.iter() {
                if let Ok(meta) = self.fs.get_node_meta(path.key()).await {

                    let mut events = Vec::new();
                    for keeper in &meta.keepers {
                        for stored_range in &keeper.ranges {
                            events.push(RangeEvent::Start(stored_range.0));
                            events.push(RangeEvent::End(stored_range.1.get_index()));
                        }
                    }

                    // let mut bad_ranges = Vec::new();
                    let mut local_chunks = self.storage.get_file_meta(&meta.filename).await.unwrap();
                    for i in (0..local_chunks.len()) {
                        let chunk = local_chunks[i];
                        events.push(RangeEvent::StoredStart(chunk.1.0, i));
                        events.push(RangeEvent::StoredEnd(chunk.1.1, i))
                    }

                    events.sort_by_key(|k| k.get_index());

                    let mut num_of_copies = 0;
                    let mut bad_segments = HashSet::new();
                    let mut stored = HashSet::new();
                    for event in events {
                        match event {
                            RangeEvent::Start(v) => num_of_copies += 1,
                            RangeEvent::End(v) => {
                                num_of_copies -= 1;
                                if num_of_copies < replication_factor {
                                    for segment in &stored {
                                        bad_segments.insert(*segment);
                                    }
                                    stored.clear();
                                }
                            },
                            RangeEvent::StoredStart(v, index) => {
                                stored.insert(index);
                                if num_of_copies < replication_factor {
                                    bad_segments.insert(index);
                                }
                            }
                            RangeEvent::StoredEnd(v, index) => {
                                stored.remove(&index);
                                if num_of_copies < replication_factor {
                                    bad_segments.insert(index);
                                }
                            }
                        }
                    }

                    for i in bad_segments {
                        let segment = local_chunks[i];

                        let keeper_n = RANDOM.take().gen_range(0..keepers.len());
                        let keeper = keepers[keeper_n];

                        // temp temp temp^2 code
                        // let mut file = self.storage.get_file(path.key(), None, None).await;
                        let mut file = self.storage.get_file(path.key(), Some(&vec![segment.1]), Some(segment.0)).await.unwrap();
                        if file.len() != 1 {
                            panic!("internal error")
                        }

                        let range = match segment.1.1 {
                            EndOfFileRange::LastByte => ByteRangeSpec::From(segment.1.0),
                            EndOfFileRange::ByteIndex(last) => ByteRangeSpec::FromTo(segment.1.0, last)
                        };
                        CLIENT.take().send_file(keeper,
                                                file.swap_remove(0).1,
                                                Range::Bytes(vec![range])).await;
                    }
                }
            }
        }
    }
}