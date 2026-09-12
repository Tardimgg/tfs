use super::{next_seq, proto_codec, upsert_value, DHTMap, DhtMapValue, DhtRecord};
use crate::services::dht_map::error::{DhtCodecError, DhtGetError, DhtPutError};
use mainline::MutableItem;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StrictDhtError {
    #[error(transparent)]
    Get(#[from] DhtGetError),
    #[error(transparent)]
    Put(#[from] DhtPutError),
    #[error(transparent)]
    Codec(#[from] DhtCodecError),
    #[error("conflicting DHT values at sequence {0}")]
    Conflict(i64),
    #[error("DHT record missing or changed before CAS")]
    Changed,
    #[error("duplicate DHT key in record")]
    DuplicateKey,
}

impl DHTMap {
    async fn strict_record(&self, key: &str) -> Result<Option<(DhtRecord, i64)>, StrictDhtError> {
        let items = self.dht.get_raw_mutable(key).await?;
        strict_latest(&items)?
            .map(|item| Ok((proto_codec::decode_record(item.value())?, *item.seq())))
            .transpose()
    }

    pub(crate) async fn get_strict<T: DhtMapValue>(
        &self,
        key: &str,
    ) -> Result<Option<(T, i64)>, StrictDhtError> {
        let Some((record, sequence)) = self.strict_record(key).await? else {
            return Ok(None);
        };
        let mut matches = record.pairs.into_iter().filter(|pair| pair.key == key);
        let Some(pair) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(StrictDhtError::DuplicateKey);
        }
        let value = pair
            .value
            .ok_or(DhtCodecError::MissingField("dht_record.pairs.value"))?;
        Ok(Some((
            T::from_dht_value(super::DhtValue::from_proto(value)?)?,
            sequence,
        )))
    }

    pub(crate) async fn compare_exchange_strict<T: DhtMapValue>(
        &self,
        key: &str,
        expected: (&T, i64),
        value: &T,
    ) -> Result<i64, StrictDhtError> {
        let (mut record, sequence) = self
            .strict_record(key)
            .await?
            .ok_or(StrictDhtError::Changed)?;
        if sequence != expected.1 {
            return Err(StrictDhtError::Changed);
        }
        if record.pairs.iter().filter(|pair| pair.key == key).count() != 1 {
            return Err(StrictDhtError::DuplicateKey);
        }
        let observed = record
            .pairs
            .iter()
            .find(|pair| pair.key == key)
            .ok_or(StrictDhtError::Changed)?;
        if observed.value.as_ref() != Some(&expected.0.to_dht_value().into_proto()) {
            return Err(StrictDhtError::Changed);
        }
        upsert_value(&mut record, key, value.to_dht_value())?;
        let next = next_seq(Some(sequence))?;
        self.put_into_dht(key, &record, next, Some(sequence))
            .await?;
        Ok(next)
    }
}

fn strict_latest(items: &[MutableItem]) -> Result<Option<&MutableItem>, StrictDhtError> {
    let Some(latest) = items.iter().max_by_key(|item| item.seq()) else {
        return Ok(None);
    };
    if items
        .iter()
        .any(|item| item.seq() == latest.seq() && item.value() != latest.value())
    {
        return Err(StrictDhtError::Conflict(*latest.seq()));
    }
    Ok(Some(latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mainline::{Bytes, SigningKey};

    #[test]
    fn denies_readback_when_newest_publications_conflict_instead_of_rolling_back() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let items = [
            MutableItem::new(key.clone(), Bytes::from_static(b"previous"), 1, None),
            MutableItem::new(key.clone(), Bytes::from_static(b"grant-a"), 2, None),
            MutableItem::new(key, Bytes::from_static(b"grant-b"), 2, None),
        ];

        let result = strict_latest(&items);

        assert!(matches!(result, Err(StrictDhtError::Conflict(2))));
    }
}
