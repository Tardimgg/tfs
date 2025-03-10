use std::net::IpAddr;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

#[derive(Debug, Serialize, Deserialize, TypedBuilder, Copy, Clone, Eq, PartialEq, Hash)]
pub struct DhtNodeId {
    pub ip: IpAddr,
    pub port: u16
}

#[derive(Copy, Clone)]
pub enum PrevSeq {
    Any,
    Seq(Option<i64>)
}