use std::fmt;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;
use moka::future::{Cache, CacheBuilder};
use crate::services::dht_map::dht_map::DHTMap;

pub struct GlobalConfig {
    dht: Arc<DHTMap>,
    cache: Cache<ConfigKey, String>
}

#[derive(Hash, Eq, PartialEq)]
pub enum ConfigKey {
    LocationOfKeepersIps,
    ReplicationFactor
}

impl ConfigKey {

    fn is_dynamic(&self) -> bool {
        match self {
            ConfigKey::LocationOfKeepersIps => true,
            ConfigKey::ReplicationFactor => true
        }
    }
}


impl ConfigKey {
    pub fn get_default_value(&self) -> String {
        match self {
            ConfigKey::LocationOfKeepersIps => "keepers".to_string(),
            ConfigKey::ReplicationFactor => "2".to_string()
        }
    }
}

impl ConfigKey{
    fn get_path(&self) -> &str {
        match self {
            ConfigKey::LocationOfKeepersIps => "keepers",
            ConfigKey::ReplicationFactor => "replicationFactor"
        }
    }
}


static CONFIG_PREFIX: &str = "/config/";
impl GlobalConfig {
    pub fn new(dht: Arc<DHTMap>) -> Self {
        let builder = Cache::builder()
            .max_capacity(1000)
            .time_to_live(Duration::from_secs(60 * 10));
        GlobalConfig {
            dht,
            cache: builder.build()
        }
    }

    pub async fn get_val_by_str(&self, key: &str) -> Option<String> {
        // добавить валидацию что key не пойдет куда не надо (config/qwe/../../home/data/secret)

        self.dht.get(&format!("{CONFIG_PREFIX}{key}")).await.unwrap_or(None).map(|v| v.0)
    }

    pub async fn get_val(&self, key: ConfigKey) -> String {
        if let Some(v) = self.cache.get(&key).await {
            return v;
        }
        if let Ok(Some(v)) = self.dht.get(&format!("{CONFIG_PREFIX}{}", key.get_path())).await {
            v.0
        } else {
            key.get_default_value()
        }
    }
}