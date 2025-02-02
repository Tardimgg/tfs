use std::any::Any;
use std::io::Write;
use std::time::Duration;
use async_trait::async_trait;
use flume::{Receiver, Sender};
use mainline::{Bytes, Dht, Id, MutableItem, SigningKey};
use mainline::async_dht::AsyncDht;
use tokio::io::AsyncReadExt;
use typed_builder::TypedBuilder;
use futures::{FutureExt, Stream, StreamExt};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sha2::digest::{DynDigest, Update};
use serde::{Deserialize, Serialize};
use tryhard::retry_fn;
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::services::dht_map::error::{CreateError, DhtGetError, KeyError, DhtPutError};

pub struct DHTMap {
    dht: AsyncDht,
}

#[derive(Serialize, Deserialize)]
pub struct DhtRecord {
    pub pairs: Vec<DistributedMapVal>,
}

#[derive(Serialize, Deserialize)]
pub struct DistributedMapVal {
    key: String,
    value: String,
}


impl DHTMap {
    pub fn new(port: u16, other: &[String]) -> Result<DHTMap, CreateError> {
        let dht = Dht::builder()
            .server()
            .bootstrap(other)
            .port(port)
            .build()?;
        let map = DHTMap {
            dht: dht.as_async()
        };

        return Ok(map);
    }

    async fn put_in_dht(&self, key: &str, record: &DhtRecord, seq: i64, cas: Option<i64>) -> Result<(), DhtPutError> {
        let signing_key = generate_key(key)?;

        if let Ok(json) = serde_json::to_string(record) {
            let mut item = MutableItem::new(signing_key, Bytes::from(json), seq, None);
            if let Some(v) = cas {
                item = item.with_cas(v);
            }

            let res = self.dht.put_mutable(item).await;
            if res.is_ok() {
                return Ok(());
            } else {
                dbg!(res.err());
                return Err(DhtPutError::XZError);
            }
        } else {
            return Err(DhtPutError::InvalidValue);
        }
    }

    async fn put_impl(&self, key: &str, value: &str) -> Result<(), DhtPutError> {
        let dht_item = self.get_mainline_dht_item(key).await?;
        if let Some(record) = dht_item {
            let mut dht_record = DhtRecord::try_from(record.clone())?;

            let exist = dht_record.pairs.iter_mut().find(|v| v.key == key);
            if let Some(prev_v) = exist {
                prev_v.value = value.to_string();

                self.put_in_dht(key, &dht_record, record.seq() + 1, Some(*record.seq())).await?;

            } else {
                dht_record.pairs.push(DistributedMapVal {
                    key: key.to_string(),
                    value: value.to_string(),
                });

                self.put_in_dht(key, &dht_record, record.seq() + 1, Some(*record.seq())).await?;
            }
        } else {
            let record = DhtRecord {
                pairs: vec![DistributedMapVal { key: key.to_string(), value: value.to_string() }]
            };

            self.put_in_dht(key, &record, 0, None).await?;
        }

        return Ok(());
    }

    async fn get_non_filtered_mainline_dht_items(&self, key: &str) -> Result<Vec<MutableItem>, DhtGetError> {
        let signing_key = generate_key(key)?;

        let mut item_stream = self.dht.get_mutable(signing_key.verifying_key().as_bytes(), None, None)?;

        let mut versions: Vec<MutableItem> = Vec::new();
        while let Some(record) = item_stream.next().await {
            versions.push(record);
        }
        return Ok(versions);
    }
    async fn get_mainline_dht_item(&self, key: &str) -> Result<Option<MutableItem>, DhtGetError> {
        let mut res = filter_mainline_items(self.get_non_filtered_mainline_dht_items(key).await?);
        if res.len() == 0 {
            return Ok(None);
        }
        let first_v = res.get(0).map(|v| v.value()).unwrap();

        if !res.iter().all(|v| v.value() == first_v) {
            return Err(DhtGetError::InternalError)
        }
        return Ok(Some(res.swap_remove(0)));
    }

    pub async fn put(&self, key: &str, value: &str) -> Result<(), DhtPutError> {
        retry_fn(|| self.put_impl(key, value))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 50))
            .await
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, DhtGetError> {

        // let (tx, rx): (Sender<i32>, Receiver<i32>) = flume::unbounded();
        // let (tx1, rx1) = flume::unbounded();
        //
        // let a = 2;
        // let mut b = rx.into_stream();

        // let mut c = rx1.into_stream();
        // let mut a: RecvStream<MutableItem> = self.dht.get_mutable(&[0u8; 32], None, None)?;

        let dht_items = self.get_mainline_dht_item(key)
            .await?;

        let record: Option<Result<DhtRecord, serde_json::Error>> = dht_items
            .map(|v| v.try_into());

        let mut versions = Vec::new();
        if let Some(pairs_result) = record {
            if let Ok(pairs) = pairs_result {
                for pair in pairs.pairs {
                    if pair.key == key {
                        versions.push(pair.value)
                    }
                }
            } else {
                return Err(DhtGetError::InternalError);
            }
        }
        // }).finalize().collect::<RecvStream<Value>>();

        // let t = a.next();

        // let t1 = a.map(|v| dbg!(v));
        // c.read_i64();
        // a.read_i32();


        // return Ok(t.await.map(|v| String::from_utf8(v.value().to_vec()).unwrap()));
        if versions.len() == 0 {
            return Ok(None);
        } else {
            if versions.len() > 1 {
                println!("err: there are more than one values for one key, key={}", key);
            }
            return Ok(Some(versions.swap_remove(0)));
        }
    }
}

fn filter_mainline_items(items: Vec<MutableItem>) -> Vec<MutableItem> {
    if items.len() == 0 {
        return vec![];
    }
    let max_seq = *items.iter().map(|v| v.seq()).max().unwrap();

    items
        .into_iter()
        .filter(|v| *v.seq() == max_seq)
        .collect()
}

fn generate_key(key: &str) -> Result<SigningKey, KeyError> {
    let mut hasher = Sha256::new();
    hasher.write(key.as_bytes())?;

    let private_key = hasher.finalize();

    Ok(SigningKey::try_from(private_key.as_slice())?)
}

impl TryFrom<MutableItem> for DhtRecord {
    type Error = serde_json::Error;

    fn try_from(value: MutableItem) -> Result<Self, Self::Error> {
        serde_json::from_slice::<DhtRecord>(value.value())
    }
}

/*
приватный ключ -> путь
публичный ключ -> все что хочет может получить -> можем его лукапть

есть проблема, что приватный ключ всегда 32 байта => хеш от пути, => коллизии => оп одному пути могут хранится совершенно разные данные
можно в данных зранить их настоящий путь

либо структуру вида

getByPublicKey(getPublicKeyBySecret(hash(path))) =
{
    "path": [ip, ip2]
    "path2": [ip, ip2]

}

даже +- секьюрно (похуй), тк это все на серверах, клиенту вернётся только ip по его пути, к которому у него есть доступ

так ок, теперь как хранить права, и инфу о загрузке

хотя, если мы теперь умеем хранить key -> value, то все готов (похуй)

 */