use std::any::Any;
use std::io::Write;
use std::str::FromStr;
use std::time::Duration;
use async_trait::async_trait;
use flume::{Receiver, Sender};
use mainline::{Bytes, Dht, Id, MutableItem, SigningKey};
use mainline::async_dht::AsyncDht;
use tokio::io::AsyncReadExt;
use typed_builder::TypedBuilder;
use futures::{FutureExt, Stream, StreamExt};
use jsonm_bugfixed::packer::{PackOptions, Packer};
use jsonm_bugfixed::unpacker::Unpacker;
use log::error;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sha2::digest::{DynDigest, Update};
use serde::{Deserialize, Serialize};
use tryhard::retry_fn;
use crate::common::default_error::DefaultError;
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::services::dht_map::error::{CreateError, DhtGetError, KeyError, DhtPutError};
use crate::services::dht_map::model::PrevSeq;

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

    async fn put_into_dht(&self, key: &str, record: &DhtRecord, seq: i64, cas: Option<i64>) -> Result<(), DhtPutError> {
        let signing_key = generate_key(key)?;

        let json = serde_json::to_string(record).map_err(|v| DhtPutError::InvalidValue)?;
        let mut item = MutableItem::new(signing_key, Bytes::from(json), seq, None);
        if let Some(v) = cas {
            item = item.with_cas(v);
        }

        self.dht.put_mutable(item).await.default_res()?;
        Ok(())
    }

    async fn put_impl(&self, key: &str, value: &str, prev_seq: PrevSeq) -> Result<(), DhtPutError> {
        let dht_item = self.get_mainline_dht_item(key).await.default_res()?;
        if let Some(record) = dht_item {
            let mut dht_record = DhtRecord::try_from(record.clone())?;

            let exist = dht_record.pairs.iter_mut().find(|v| v.key == key);
            if let Some(prev_v) = exist {
                prev_v.value = value.to_string();
            } else {
                dht_record.pairs.push(DistributedMapVal {
                    key: key.to_string(),
                    value: value.to_string(),
                });
            }
            match prev_seq {
                PrevSeq::Any => {
                    self.put_into_dht(key, &dht_record, *record.seq() + 1, Some(*record.seq())).await?;
                }
                PrevSeq::Seq(seq) => {
                    self.put_into_dht(key, &dht_record, seq.map(|v| v + 1).unwrap_or(0), seq).await?;
                }
            }

        } else {
            let record = DhtRecord {
                pairs: vec![DistributedMapVal { key: key.to_string(), value: value.to_string() }]
            };

            match prev_seq {
                PrevSeq::Any => {
                    self.put_into_dht(key, &record, 0, None).await?;
                }
                PrevSeq::Seq(seq) => {
                    self.put_into_dht(key, &record, seq.map(|v| v + 1).unwrap_or(0), seq).await?;
                }
            }
        }

        Ok(())
    }

    async fn get_non_filtered_mainline_dht_items(&self, key: &str) -> Result<Vec<MutableItem>, DhtGetError> {
        let signing_key = generate_key(key)?;

        let mut item_stream = self.dht.get_mutable(signing_key.verifying_key().as_bytes(), None, None)?;

        let mut versions: Vec<MutableItem> = Vec::new();
        while let Some(record) = item_stream.next().await {
            versions.push(record);
        }
        Ok(versions)
    }
    async fn get_mainline_dht_item(&self, key: &str) -> Result<Option<MutableItem>, DhtGetError> {
        let mut non_filtered = self.get_non_filtered_mainline_dht_items(key).await?;

        let mut fallback = 0;
        loop {
            let mut res = filter_mainline_items(&non_filtered);
            if res.len() == 0 {
                return Ok(None);
            }
            let first_v = res.get(0).map(|v| v.value()).unwrap();

            if !res.iter().all(|v| v.value() == first_v) {
                res.iter().for_each(|v| error!("err {}", String::from_utf8_lossy(v.value())));
                // кажется в этом случае все запросы с наибольшем seq неуспешны, нужно найти наиболее младшего в единственном экземпляре
                // (и возможно обновить его), тк хз сколько он будет хранится
                error!("An unexpected value was received from dht, rollback = {}", fallback);
                non_filtered.retain(|v| v.seq() < res.get(0).unwrap().seq());
                fallback += 1;
                if non_filtered.is_empty() {
                    break;
                }
                // return Err(DhtGetError::InternalError("".to_string()))
            } else {
                if fallback > 0 {
                    error!("fallback scheme with a rollback of {} entries is used", fallback)
                }
                return Ok(Some(res.swap_remove(0)))
            }
        }
        if fallback > 0 {
            error!("not found valid data after {} attempts", fallback)
        }
        return Ok(None);
    }

    pub async fn put(&self, key: &str, value: &str) -> Result<(), DhtPutError> {
        let mut packer = Packer::new();
        let options = PackOptions::new();
        let compressed = packer.pack_string(value, &options).default_res()?;
        let compressed_string = compressed.to_string();
        retry_fn(|| self.put_impl(key, &compressed_string, PrevSeq::Any))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 50))
            .await
    }

    pub async fn put_with_seq(&self, key: &str, value: &str, prev_seq: PrevSeq) -> Result<(), DhtPutError> {
        let mut packer = Packer::new();
        let options = PackOptions::new();
        let compressed = packer.pack_string(value, &options).default_res()?;
        let compressed_string = compressed.clone().to_string();

        retry_fn(|| self.put_impl(key, &compressed_string, prev_seq))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 50))
            .await
    }

    pub async fn get(&self, key: &str) -> Result<Option<(String, i64)>, DhtGetError> {
        let dht_items = self.get_mainline_dht_item(key)
            .await?;

        let record: Option<(Result<DhtRecord, serde_json::Error>, i64)> = dht_items
            .map(|v| {
                let seq = *v.seq();
                (v.try_into(), seq)
            });

        let mut versions = Vec::new();
        if let Some((pairs_result, seq)) = record {
            if let Ok(pairs) = pairs_result {
                for pair in pairs.pairs {
                    if pair.key == key {
                        versions.push((pair.value, seq))
                    }
                }
            } else {
                return Err(DhtGetError::InternalError("".to_string()));
            }
        }

        if versions.len() == 0 {
            Ok(None)
        } else {
            if versions.len() > 1 {
                println!("err: there is more than one values for one key, key={}, values={:?}", key, versions);
            }
            let compressed = versions.swap_remove(0);
            let mut unpacker = Unpacker::new();
            let raw = unpacker.unpack_string(&Value::from_str(&compressed.0).unwrap()).default_res()?;
            Ok(Some((raw.to_string(), compressed.1)))
        }
    }
}

fn filter_mainline_items(items: &[MutableItem]) -> Vec<MutableItem> {
    if items.len() == 0 {
        return vec![];
    }
    let max_seq = *items.iter().map(|v| v.seq()).max().unwrap();

    items
        .iter()
        .filter(|v| *v.seq() == max_seq)
        .map(|v| v.clone())
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