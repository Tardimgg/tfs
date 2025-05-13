use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use dashmap::DashMap;
use moka::future::{Cache, CacheBuilder};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use crate::config::global_config;
use crate::services::dht_map::dht_map::DHTMap;

pub struct GlobalConfig {
    dht: Arc<DHTMap>,
    cache: Cache<ConfigKey, String>,
    initial_overrides: DashMap<ConfigKey, String>
}

#[derive(Hash, Eq, PartialEq, EnumIter, Copy, Clone, Debug)]
pub enum ConfigKey {
    LocationOfKeepersIps,
    ReplicationFactor,
    DataPath,
    DataFolderId,
    DataMetaPath,
    DataMetaFolderId,
    TempFolder,
    HomePath,
    HomeFolderId,
    RebacPath,
    RebacFolderId,
    UsersPath,
    UsersFolderId,
    FsNodeIdMappingPath,
    UserUidMappingPath,
    NumberOfStoredVersions,
    CacheTTLInSeconds,
    CacheCapacity,
    SelfUid,
    SecretSigningKey,
    AnonymousUid
}

impl ConfigKey {

    fn is_dynamic(&self) -> bool {
        match self {
            ConfigKey::LocationOfKeepersIps => true,
            ConfigKey::ReplicationFactor => true,
            ConfigKey::DataPath => false,
            ConfigKey::DataMetaPath => false,
            ConfigKey::TempFolder => false,
            ConfigKey::HomePath => false,
            ConfigKey::RebacPath => false,
            ConfigKey::NumberOfStoredVersions => true,
            ConfigKey::CacheTTLInSeconds => false,
            ConfigKey::CacheCapacity => false,
            ConfigKey::UsersPath => false,
            ConfigKey::FsNodeIdMappingPath => false,
            ConfigKey::UserUidMappingPath => false,
            ConfigKey::DataFolderId => false,
            ConfigKey::DataMetaFolderId => false,
            ConfigKey::HomeFolderId => false,
            ConfigKey::RebacFolderId => false,
            ConfigKey::UsersFolderId => false,
            ConfigKey::SelfUid => true,
            ConfigKey::SecretSigningKey => true,
            ConfigKey::AnonymousUid => true
        }
    }
}


impl ConfigKey {
    pub fn get_default_value(&self) -> String {
        match self {
            ConfigKey::LocationOfKeepersIps => "keepers".to_string(),
            ConfigKey::ReplicationFactor => "2".to_string(),
            ConfigKey::DataPath => "data".to_string(),
            ConfigKey::DataMetaPath => "data_meta".to_string(),
            ConfigKey::TempFolder => "temp".to_string(),
            ConfigKey::HomePath => "home".to_string(),
            ConfigKey::RebacPath => "rebac".to_string(),
            ConfigKey::NumberOfStoredVersions => "5".to_string(),
            ConfigKey::CacheTTLInSeconds => (60 * 10).to_string(),
            ConfigKey::CacheCapacity => "1000".to_string(),
            ConfigKey::UsersPath => "users".to_string(),
            ConfigKey::FsNodeIdMappingPath => "node_id_mapping".to_string(),
            ConfigKey::UserUidMappingPath => "user_uid_mapping".to_string(),
            ConfigKey::DataFolderId => "1".to_string(),
            ConfigKey::DataMetaFolderId => "2".to_string(),
            ConfigKey::HomeFolderId => "3".to_string(),
            ConfigKey::RebacFolderId => "4".to_string(),
            ConfigKey::UsersFolderId => "5".to_string(),
            ConfigKey::SelfUid => "1".to_string(),
            ConfigKey::SecretSigningKey => "some-secret".to_string(),
            ConfigKey::AnonymousUid => "2".to_string()
        }
    }
}

impl ConfigKey{
    fn get_path(&self) -> &str {
        match self {
            ConfigKey::LocationOfKeepersIps => "keepers",
            ConfigKey::ReplicationFactor => "replicationFactor",
            ConfigKey::DataPath => "data_path",
            ConfigKey::DataMetaPath => "data_meta_path",
            ConfigKey::TempFolder => "temp_folder",
            ConfigKey::HomePath => "fs_path",
            ConfigKey::RebacPath => "rebac_path",
            ConfigKey::NumberOfStoredVersions => "number_of_stored_versions",
            ConfigKey::CacheTTLInSeconds => "cache_ttl_in_seconds",
            ConfigKey::CacheCapacity => "cache_capacity",
            ConfigKey::UsersPath => "users_path",
            ConfigKey::FsNodeIdMappingPath => "fs_node_id_mapping",
            ConfigKey::UserUidMappingPath => "user_uid_mapping",
            ConfigKey::DataFolderId => "data_folder_id",
            ConfigKey::DataMetaFolderId => "data_meta_folder_id",
            ConfigKey::HomeFolderId => "home_folder_id",
            ConfigKey::RebacFolderId => "rebac_folder_id",
            ConfigKey::UsersFolderId => "users_folder_id",
            ConfigKey::SelfUid => "self_uid",
            ConfigKey::SecretSigningKey => "secret_signing_secret",
            ConfigKey::AnonymousUid => "anonymous_uid"
        }
    }
}


static CONFIG_PREFIX: &str = "/config/";
impl GlobalConfig {
    pub async fn new(dht: Arc<DHTMap>) -> Self {
        let mut global_config = GlobalConfig {
            dht,
            cache: Cache::new(1),
            initial_overrides: Default::default(),
        };

        let mut overrides = DashMap::default();
        for key in ConfigKey::iter() {
            overrides.insert(key, global_config.get_fresh_val(key).await);
        }

        let cache_capacity = global_config.get_fresh_or_default(ConfigKey::CacheCapacity).await;
        let cache_ttl = global_config.get_fresh_or_default(ConfigKey::CacheTTLInSeconds).await;

        let builder = Cache::builder()
            .max_capacity(cache_capacity)
            .time_to_live(Duration::from_secs(cache_ttl));

        global_config.initial_overrides = overrides;
        global_config.cache = builder.build();
        global_config
    }

    pub async fn get_val_by_str(&self, key: &str) -> Option<String> {
        self.dht.get(&format!("{CONFIG_PREFIX}{key}")).await.unwrap_or(None).map(|v| v.0)
    }

    pub async fn get_val(&self, key: ConfigKey) -> String {
        if !key.is_dynamic() {
            return if let Some(v) = self.initial_overrides.get(&key) {
                v.value().to_string()
            } else {
                key.get_default_value()
            }
        }

        if let Some(v) = self.cache.get(&key).await {
            return v;
        }
        let val = self.get_fresh_val(key).await;
        self.cache.insert(key, val.clone()).await;
        val
    }

    async fn get_fresh_or_default<T: FromStr>(&self, key: ConfigKey) -> T {
        T::from_str(&self.get_fresh_val(key).await)
            .unwrap_or(T::from_str(&key.get_default_value())
                .map_err(|v| format!("default value of key {:?} is invalid", key))
                .unwrap()
            )
    }

    async fn get_fresh_val(&self, key: ConfigKey) -> String {
        if let Ok(Some(v)) = self.dht.get(&format!("{CONFIG_PREFIX}{}", key.get_path())).await {
            v.0
        } else {
            key.get_default_value()
        }
    }
}