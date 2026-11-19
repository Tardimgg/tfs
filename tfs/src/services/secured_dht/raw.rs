use std::io::Write;

use futures::StreamExt;
use mainline::async_dht::AsyncDht;
use mainline::{Bytes, MutableItem, RevocationSnapshot, SigningKey};
use sha2::{Digest, Sha256};

use crate::services::dht_map::codec::MAX_DHT_VALUE_SIZE;
use crate::services::dht_map::error::{DhtGetError, DhtPutError, KeyError};

pub(super) struct RawDht {
    dht: AsyncDht,
}

impl RawDht {
    pub(super) fn new(dht: AsyncDht) -> Self {
        Self { dht }
    }

    pub(super) async fn get(&self, key: &str) -> Result<Vec<MutableItem>, DhtGetError> {
        let signing_key = generate_key(key)?;
        let mut item_stream = self.dht
            .get_mutable(signing_key.verifying_key().as_bytes(), None, None)?;
        let mut versions = Vec::new();
        while let Some(record) = item_stream.next().await {
            versions.push(record);
        }
        Ok(versions)
    }

    pub(super) async fn put(
        &self,
        key: &str,
        value: Vec<u8>,
        sequence: i64,
        cas: Option<i64>,
    ) -> Result<(), DhtPutError> {
        self.dht.put_mutable(mutable_item(key, value, sequence, cas)?).await?;
        Ok(())
    }

    pub(super) async fn install_protected(
        &self,
        key: &str,
        value: Vec<u8>,
        sequence: i64,
    ) -> Result<bool, DhtPutError> {
        Ok(self.dht.install_protected_mutable(mutable_item(key, value, sequence, None)?).await?)
    }

    pub(super) async fn protected(&self, key: &str) -> Result<Option<MutableItem>, DhtGetError> {
        let signing_key = generate_key(key)?;
        let target = MutableItem::target_from_key(signing_key.verifying_key().as_bytes(), &None);
        Ok(self.dht.protected_mutable(target).await?)
    }

    pub(super) async fn replace_dtls_revocations(
        &self,
        snapshot: RevocationSnapshot,
    ) -> Result<bool, mainline::Error> {
        self.dht.replace_dtls_revocations(snapshot).await
    }

    pub(super) async fn start_serving_if_protected_matches(
        &self,
        key: &str,
        value: Vec<u8>,
        sequence: i64,
    ) -> Result<bool, DhtPutError> {
        Ok(self.dht.start_serving_if_protected_matches(mutable_item(key, value, sequence, None)?).await?)
    }

    #[cfg(test)]
    pub(super) async fn local_addr(&self) -> Result<std::net::SocketAddr, mainline::Error> {
        self.dht.local_addr().await
    }
}

pub(crate) fn generate_key(key: &str) -> Result<SigningKey, KeyError> {
    let mut hasher = Sha256::new();
    hasher.write(key.as_bytes())?;
    let private_key = hasher.finalize();
    Ok(SigningKey::try_from(private_key.as_slice())?)
}

fn mutable_item(
    key: &str,
    value: Vec<u8>,
    sequence: i64,
    cas: Option<i64>,
) -> Result<MutableItem, DhtPutError> {
    validate_dht_value_size(value.len())?;
    let item = MutableItem::new(generate_key(key)?, Bytes::from(value), sequence, None);
    Ok(match cas {
        Some(cas) => item.with_cas(cas),
        None => item,
    })
}

fn validate_dht_value_size(size: usize) -> Result<(), DhtPutError> {
    if size > MAX_DHT_VALUE_SIZE {
        return Err(DhtPutError::ValueTooLarge { size, max: MAX_DHT_VALUE_SIZE });
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
}
