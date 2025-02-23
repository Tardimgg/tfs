use std::fmt;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use crate::services::dht_map::dht_map::DHTMap;

pub struct GlobalConfig {
    dht: Arc<DHTMap>
}

pub enum ConfigKey {
    LocationOfKeepersIps,
    ReplicationFactor
}

impl ConfigKey {
    pub fn get_default_value(&self) -> String {
        match self {
            ConfigKey::LocationOfKeepersIps => "keepers".to_string(),
            ConfigKey::ReplicationFactor => "3".to_string()
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
        GlobalConfig {
            dht
        }
    }

    pub async fn get_val_by_str(&self, key: &str) -> Option<String> {
        // добавить валидацию что key не пойдет куда не надо (config/qwe/../../home/data/secret)

        self.dht.get(&format!("{CONFIG_PREFIX}{key}")).await.unwrap_or(None)
    }

    pub async fn get_val(&self, key: ConfigKey) -> String {
        // добавить валидацию что key не пойдет куда не надо (config/qwe/../../home/data/secret)

        if let Ok(Some(v)) = self.dht.get(&format!("{CONFIG_PREFIX}{}", key.get_path())).await {
            v
        } else {
            key.get_default_value()
        }
    }
}