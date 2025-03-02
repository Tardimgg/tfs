use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Index;
use std::panic;
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
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::common::state_update::StateUpdate;
use crate::config::global_config::{ConfigKey, GlobalConfig};
use crate::heartbeat::models::RangeEvent;
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::DhtNodeId;
use crate::services::file_storage::file_storage::FileStorage;
use crate::services::file_storage::model::{ChunkVersion, NodeType};
use crate::services::internal_communication::InternalCommunication;
use crate::services::virtual_fs::models::{StoredFileRange, StoredFileRangeEnd};
use crate::services::virtual_fs::virtual_fs::VirtualFS;
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
            while let Some(update) = update_receiver.recv().await {
                match update {
                    StateUpdate::NewFile(path) => {
                        map_ref.insert(path, SystemTime::now())
                    }
                };
            }
        });

        self.init_db().await;

        let mut interval = tokio::time::interval(Duration::from_secs(20));


        loop {
            interval.tick().await;
            // panic::catch_unwind(|| async {

                self.replication_files().await;

            // });
            if self.cancellation_token.is_cancelled() {
                updater.abort();
                break;
            }
        }
        tokio::join!(updater);
    }


    async fn init_db(&self) {
        self.scan_folder("/").await;
    }

    async fn scan_folder(&self, root: &str) {
        let folder = self.storage.get_folder_content(root).await.unwrap();
        for file in folder.files {
            match file.node_type {
                NodeType::File => {
                    self.state.insert(file.name, SystemTime::now());
                }
                NodeType::Folder => {
                    Box::pin(self.scan_folder(&file.name)).await;
                }
            }
        }

    }

    async fn replication_files(&self) {
        self.fs.init().await; // тут подумать, может перейти на анонсы dht, может делать только при определённых условиях и каким то таймаутом

        // если в dht версия ниже, то это невозможно, бекапим запись. Если отсутствует или больше, то допустим что ок
        println!("{:?}", self.state);
        let keepers_result = self.fs.get_all_keepers().await.map(|mut keepers| {
            if let Some(index) = keepers.iter().position(|v| v == self.fs.get_id()) {
                keepers.swap_remove(index);
            }
            return keepers;
        });
        println!("keepers: {:?}", keepers_result);

        // минус один нужно перенести в более подходящее место
        let replication_factor = usize::from_str(&self.config.get_val(ConfigKey::ReplicationFactor).await).unwrap() - 1; // себя не считаем
        if let Ok(keepers) = keepers_result {
            if keepers.len() == 0 {
                return
            }
            for path in self.state.iter() {
                let mut events = Vec::new();

                let mut self_updated = false;
                if let Ok(meta) = self.fs.get_node_meta(path.key()).await {
                    for keeper in &meta.keepers {
                        if keeper.id != *self.fs.get_id() {
                            println!("keeper file {} info: {:?}", path.key(), keeper);
                            let real_stored = CLIENT.take().get_stored_parts_of_file(path.key(), keeper.id).await;
                            if let Ok(stored) = real_stored {

                                // impl check equals stored and dht data

                                for stored_range in &keeper.ranges {
                                    events.push(RangeEvent::Start(stored_range.from));
                                    events.push(RangeEvent::End(stored_range.to.get_index()));
                                }
                            } else {
                                println!("impl remove from dht")
                                // remove from dht
                            }
                        } else {
                            self.update_dht_state(path.key(), &keeper.ranges).await;
                            self_updated = true;
                        }
                    }
                }
                if !self_updated {
                    self.update_dht_state(path.key(), &vec![]).await;
                }


                let mut local_chunks = self.storage.get_file_meta(&path.key()).await.unwrap();
                for i in (0..local_chunks.len()) {
                    let chunk = local_chunks[i];
                    events.push(RangeEvent::StoredStart(chunk.from, i));
                    events.push(RangeEvent::StoredEnd(chunk.to, i))
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
                    // нужно подумать как тут не отослать узлу, у которого уже есть этот чанк
                    let segment = local_chunks[i];

                    let keeper_n = RANDOM.take().gen_range(0..keepers.len());
                    let keeper = keepers[keeper_n];

                    // temp temp temp^2 code
                    // let mut file = self.storage.get_file(path.key(), None, None).await;

                    let chunk_version = ChunkVersion(segment.version);
                    let mut file = self.storage.get_file(path.key(), Some(&vec![FileRange::from(segment)]), Some(chunk_version)).await.unwrap();
                    if file.len() != 1 {
                        panic!("internal error")
                    }

                    let range = match segment.to {
                        StoredFileRangeEnd::EndOfRange(last) => ByteRangeSpec::FromTo(segment.from, last),
                        StoredFileRangeEnd::EnfOfFile(_) =>  ByteRangeSpec::From(segment.from)
                    };
                    println!("send file {} with range {} {:?}", path.key(), segment.from, segment.to);
                    CLIENT.take().send_file(path.key(), keeper,
                                            file.swap_remove(0).1,
                                            Range::Bytes(vec![range])).await;
                }
            }
        }
    }

    async fn update_dht_state(&self, filename: &str, dht_state: &[StoredFileRange]) {
        let mut to_add_to_dht = Vec::new();
        let stored_local = self.storage.get_file_meta(filename).await.unwrap();
        let mut dht_chunks = dht_state.iter().map(|v| *v).collect::<HashSet<StoredFileRange>>();

        for local_range in stored_local {
            if !dht_chunks.contains(&local_range) {
                to_add_to_dht.push(local_range)
            } else {
                dht_chunks.remove(&local_range);
            }
        }
        let to_delete: HashSet<StoredFileRange> = dht_chunks.iter().map(|v| *v).collect();

        println!("state update: file: {} delete {:?}, add: {:?}", filename, to_delete, to_add_to_dht);
        if to_delete.len() != 0 || to_add_to_dht.len() != 0 {
            let res = self.fs.update_state(filename, &to_add_to_dht.iter().map(|v| *v).collect(), &to_delete).await;
            if let Err(err) = res {
                println!("err: {}", err.to_string());
            }
        }


    }
}