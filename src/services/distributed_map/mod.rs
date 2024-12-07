use std::time::Duration;
use async_trait::async_trait;
use tryhard::retry_fn;
use crate::common::retry_police::FixedBackoffWithJitter;
use crate::services::distributed_map::error::{GetError, PutError};

pub mod error;

#[async_trait]
pub trait DistributedMap {
    async fn put(&self, key: &str, value: &str) -> Result<(), PutError>;
    async fn get(&self, key: &str) -> Result<Vec<String>, GetError>;
}

