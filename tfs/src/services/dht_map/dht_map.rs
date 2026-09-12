use std::any::Any;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use async_trait::async_trait;
use flume::{Receiver, Sender};
use mainline::{Bytes, Dht, DtlsConfig, Id, MutableItem, RevocationSnapshot, SigningKey};
use mainline::async_dht::AsyncDht;
use mainline::server::{DhtServer, DhtServerSettings, MutableItemValidator};
use tokio::io::AsyncReadExt;
use typed_builder::TypedBuilder;
use futures::{FutureExt, Stream, StreamExt};
use log::error;
use sha2::{Digest, Sha256};
use sha2::digest::{DynDigest, Update};
use tryhard::retry_fn;
use crate::common::default_error::DefaultError;
use crate::common::retry_police::fixed_backoff_with_jitter::FixedBackoffWithJitter;
use crate::services::dht_map::codec as proto_codec;
use crate::services::dht_map::codec::{DhtMapValue, DhtRecord, DhtValue, DistributedMapVal, MAX_DHT_VALUE_SIZE};
use crate::services::dht_map::error::{CreateError, DhtCodecError, DhtGetError, KeyError, DhtPutError};
use crate::services::dht_map::model::PrevSeq;

pub struct DHTMap {
    dht: AsyncDht,
    serving: Arc<AtomicBool>,
    revocations: mainline::RevocationStore,
    security_failure: tokio_util::sync::CancellationToken,
}

impl DHTMap {
    pub fn new(port: u16, other: &[String], dtls: DtlsConfig, mutable_item_validator: Arc<dyn MutableItemValidator>) -> Result<DHTMap, CreateError> {
        let serving = Arc::new(AtomicBool::new(false));
        let revocations = dtls.revocations().clone();
        let server = DhtServer::new(&DhtServerSettings {
            mutable_item_validator: Some(mutable_item_validator),
            request_gate: Some(serving.clone()),
            ..DhtServerSettings::default()
        });
        let dht = Dht::builder()
            .custom_server(Box::new(server))
            .bootstrap(other)
            .port(port)
            .dtls(dtls)
            .build()?;
        let map = DHTMap {
            dht: dht.as_async(),
            serving,
            revocations,
            security_failure: tokio_util::sync::CancellationToken::new(),
        };

        return Ok(map);
    }

    pub(crate) async fn start_serving_if_protected_matches(&self, key: &str, value: Vec<u8>, sequence: i64) -> Result<bool, DhtPutError> {
        self.check_security()?;
        validate_dht_value_size(value.len())?;
        let item = MutableItem::new(generate_key(key)?, Bytes::from(value), sequence, None);
        let started = self.dht.start_serving_if_protected_matches(item).await?;
        if let Err(error) = self.check_security() {
            self.stop_serving();
            return Err(error.into());
        }
        Ok(started)
    }

    pub(crate) fn stop_serving(&self) {
        self.serving.store(false, Ordering::Release);
    }

    pub(crate) fn fail_security(&self) {
        self.revocations.fail_security();
        self.security_failure.cancel();
        self.stop_serving();
    }

    pub(crate) fn check_security(&self) -> Result<(), mainline::Error> {
        if self.revocations.security_failed() {
            Err(mainline::Error::SecurityFailed)
        } else {
            Ok(())
        }
    }

    async fn application_operation<T>(&self, operation: impl std::future::Future<Output = T>) -> Result<T, mainline::Error> {
        self.check_security()?;
        tokio::select! {
            biased;
            _ = self.security_failure.cancelled() => Err(mainline::Error::SecurityFailed),
            result = operation => {
                self.check_security()?;
                Ok(result)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_serving(&self) -> bool {
        self.serving.load(Ordering::Acquire)
    }

    pub(crate) async fn local_addr(&self) -> Result<std::net::SocketAddr, mainline::Error> {
        self.dht.local_addr().await
    }

    pub(crate) async fn get_raw_mutable(&self, key: &str) -> Result<Vec<MutableItem>, DhtGetError> {
        if key == crate::services::dtls_revocation_registry::REVOCATION_DHT_KEY {
            self.get_non_filtered_mainline_dht_items(key).await
        } else {
            self.application_operation(self.get_non_filtered_mainline_dht_items(key)).await?
        }
    }

    pub(crate) async fn put_raw_mutable(&self, key: &str, value: Vec<u8>, sequence: i64, cas: Option<i64>) -> Result<(), DhtPutError> {
        validate_dht_value_size(value.len())?;
        let mut item = MutableItem::new(generate_key(key)?, Bytes::from(value), sequence, None);
        if let Some(cas) = cas {
            item = item.with_cas(cas);
        }
        if key == crate::services::dtls_revocation_registry::REVOCATION_DHT_KEY {
            self.dht.put_mutable(item).await?;
        } else {
            self.application_operation(self.dht.put_mutable(item)).await??;
        }
        Ok(())
    }

    pub(crate) async fn replace_dtls_revocations(&self, snapshot: RevocationSnapshot) -> Result<bool, mainline::Error> {
        self.dht.replace_dtls_revocations(snapshot).await
    }

    pub(crate) async fn install_protected_raw_mutable(&self, key: &str, value: Vec<u8>, sequence: i64) -> Result<bool, DhtPutError> {
        validate_dht_value_size(value.len())?;
        let item = MutableItem::new(generate_key(key)?, Bytes::from(value), sequence, None);
        Ok(self.dht.install_protected_mutable(item).await?)
    }

    pub(crate) async fn get_local_protected_raw_mutable(&self, key: &str) -> Result<Option<MutableItem>, DhtGetError> {
        let signing_key = generate_key(key)?;
        let target = MutableItem::target_from_key(signing_key.verifying_key().as_bytes(), &None);
        Ok(self.dht.protected_mutable(target).await?)
    }

    async fn put_into_dht(&self, key: &str, record: &DhtRecord, seq: i64, cas: Option<i64>) -> Result<(), DhtPutError> {
        self.check_security()?;
        let signing_key = generate_key(key)?;

        let encoded = proto_codec::encode_record(record)?;
        validate_dht_value_size(encoded.len())?;
        let mut item = MutableItem::new(signing_key, Bytes::from(encoded), seq, None);
        if let Some(v) = cas {
            item = item.with_cas(v);
        }

        self.application_operation(self.dht.put_mutable(item)).await??;
        Ok(())
    }

    async fn put_impl<T: DhtMapValue>(&self, key: &str, value: &T, prev_seq: PrevSeq) -> Result<(), DhtPutError> {
        let dht_item = self.get_mainline_dht_item(key).await.default_res()?;
        let value = value.to_dht_value();
        if let Some(record) = dht_item {
            let mut dht_record = proto_codec::decode_record(record.value())?;
            upsert_value(&mut dht_record, key, value)?;
            match prev_seq {
                PrevSeq::Any => {
                    self.put_into_dht(key, &dht_record, next_seq(Some(*record.seq()))?, Some(*record.seq())).await?;
                }
                PrevSeq::Seq(seq) => {
                    self.put_into_dht(key, &dht_record, next_seq(seq)?, seq).await?;
                }
            }

        } else {
            let mut record = DhtRecord { pairs: vec![] };
            upsert_value(&mut record, key, value)?;

            match prev_seq {
                PrevSeq::Any => {
                    self.put_into_dht(key, &record, 0, None).await?;
                }
                PrevSeq::Seq(seq) => {
                    self.put_into_dht(key, &record, next_seq(seq)?, seq).await?;
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
        let mut non_filtered = self.application_operation(self.get_non_filtered_mainline_dht_items(key)).await??;

        let mut fallback = 0;
        loop {
            let mut res = filter_mainline_items(&non_filtered);
            if res.len() == 0 {
                return Ok(None);
            }
            let first_v = res.get(0).map(|v| v.value()).unwrap();

            if !res.iter().all(|v| v.value() == first_v) {
                res.iter().for_each(|v| error!("conflicting DHT value: seq={}, size={}", v.seq(), v.value().len()));
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

    pub(crate) async fn put<T: DhtMapValue>(&self, key: &str, value: &T) -> Result<(), DhtPutError> {
        self.application_operation(retry_fn(|| self.put_impl(key, value, PrevSeq::Any))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 50))
        ).await?
    }

    pub(crate) async fn put_with_seq<T: DhtMapValue>(&self, key: &str, value: &T, prev_seq: PrevSeq) -> Result<(), DhtPutError> {
        self.application_operation(retry_fn(|| self.put_impl(key, value, prev_seq))
            .retries(3)
            .custom_backoff(FixedBackoffWithJitter::new(Duration::from_secs(2), 50))
        ).await?
    }

    pub(crate) async fn get<T: DhtMapValue>(&self, key: &str) -> Result<Option<(T, i64)>, DhtGetError> {
        self.get_value(key)
            .await?
            .map(|(value, seq)| T::from_dht_value(value).map(|value| (value, seq)))
            .transpose()
            .map_err(DhtGetError::from)
    }

    pub(crate) async fn get_value(&self, key: &str) -> Result<Option<(DhtValue, i64)>, DhtGetError> {
        let dht_items = self.get_mainline_dht_item(key)
            .await?;

        let record = dht_items
            .map(|v| {
                let seq = *v.seq();
                (proto_codec::decode_record(v.value()), seq)
            });

        let mut versions = Vec::new();
        if let Some((pairs_result, seq)) = record {
            let pairs = pairs_result?;
            for pair in pairs.pairs {
                if pair.key == key {
                    let value = pair.value.ok_or(DhtCodecError::MissingField("dht_record.pairs.value"))?;
                    versions.push((DhtValue::from_proto(value)?, seq))
                }
            }
        }

        if versions.len() == 0 {
            Ok(None)
        } else {
            if versions.len() > 1 {
                error!("there is more than one value for DHT key {key}");
            }
            Ok(Some(versions.swap_remove(0)))
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

pub(crate) fn generate_key(key: &str) -> Result<SigningKey, KeyError> {
    let mut hasher = Sha256::new();
    hasher.write(key.as_bytes())?;

    let private_key = hasher.finalize();

    Ok(SigningKey::try_from(private_key.as_slice())?)
}

fn next_seq(seq: Option<i64>) -> Result<i64, DhtPutError> {
    match seq {
        Some(seq) => seq.checked_add(1).ok_or(DhtPutError::SequenceOverflow(seq)),
        None => Ok(0),
    }
}

fn validate_dht_value_size(size: usize) -> Result<(), DhtPutError> {
    if size > MAX_DHT_VALUE_SIZE {
        return Err(DhtPutError::ValueTooLarge {
            size,
            max: MAX_DHT_VALUE_SIZE,
        });
    }
    Ok(())
}

fn upsert_value(record: &mut DhtRecord, key: &str, value: DhtValue) -> Result<(), DhtCodecError> {
    if let Some(previous) = record.pairs.iter_mut().find(|pair| pair.key == key) {
        let previous_value = DhtValue::from_proto(
            previous
                .value
                .clone()
                .ok_or(DhtCodecError::MissingField("dht_record.pairs.value"))?,
        )?;
        if previous_value.kind_name() != value.kind_name() {
            return Err(DhtCodecError::UnexpectedValueType {
                expected: previous_value.kind_name(),
                actual: value.kind_name(),
            });
        }
        previous.value = Some(value.into_proto());
    } else {
        record.pairs.push(DistributedMapVal {
            key: key.to_string(),
            value: Some(value.into_proto()),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_dht_value_size_boundary() {
        assert!(validate_dht_value_size(MAX_DHT_VALUE_SIZE).is_ok());
        assert!(matches!(
            validate_dht_value_size(MAX_DHT_VALUE_SIZE + 1),
            Err(DhtPutError::ValueTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_sequence_overflow() {
        assert_eq!(next_seq(None).unwrap(), 0);
        assert_eq!(next_seq(Some(41)).unwrap(), 42);
        assert!(matches!(
            next_seq(Some(i64::MAX)),
            Err(DhtPutError::SequenceOverflow(i64::MAX))
        ));
    }

    #[test]
    fn preserves_existing_value_type() {
        let mut record = DhtRecord {
            pairs: vec![DistributedMapVal {
                key: "key".to_string(),
                value: Some(DhtValue::Text("value".to_string()).into_proto()),
            }],
        };

        assert!(matches!(
            upsert_value(&mut record, "key", DhtValue::UnsignedInteger(42)),
            Err(DhtCodecError::UnexpectedValueType { .. })
        ));
        assert!(matches!(
            DhtValue::from_proto(record.pairs[0].value.clone().unwrap()).unwrap(),
            DhtValue::Text(value) if value == "value"
        ));
    }
}
