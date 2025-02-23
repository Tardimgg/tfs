use std::net::IpAddr;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

#[derive(Debug, Serialize, Deserialize, TypedBuilder, Copy, Clone)]
pub struct DhtNodeId {
    pub ip: IpAddr,
    pub port: u16
}