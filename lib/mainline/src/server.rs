//! Modules needed only for nodes running in server mode (not read-only).

pub mod peers;
pub mod tokens;

use std::{
    collections::HashMap,
    net::SocketAddr,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use bytes::Bytes;
use lru::LruCache;
use tracing::debug;
use moka::sync::Cache;
use std::time::Duration;



use crate::{
    common::{
        validate_immutable, AnnouncePeerRequestArguments, ErrorSpecific, FindNodeRequestArguments,
        FindNodeResponseArguments, GetImmutableResponseArguments, GetMutableResponseArguments,
        GetPeersRequestArguments, GetPeersResponseArguments, GetValueRequestArguments, Id,
        MutableItem, NoMoreRecentValueResponseArguments, NoValuesResponseArguments,
        PingResponseArguments, PutImmutableRequestArguments, PutMutableRequestArguments,
        PutRequest, PutRequestSpecific, RequestSpecific, RequestTypeSpecific, ResponseSpecific,
    },
    rpc::Rpc,
};

use peers::PeersStore;
use tokens::Tokens;

// Stored data in server mode.
pub const MAX_INFO_HASHES: usize = 2000;
pub const MAX_PEERS: usize = 500;
pub const MAX_VALUES: usize = 1000;

/// Dht server that can handle incoming rpc requests
pub trait Server: std::fmt::Debug + Send + Sync {
    /// Return a gate that must be open before inbound requests enter RPC bookkeeping.
    fn inbound_request_gate(&self) -> Option<Arc<AtomicBool>> {
        None
    }

    /// Return the validator used to admit protected mutable control-plane requests while gated.
    fn gated_mutable_item_validator(&self) -> Option<Arc<dyn MutableItemValidator>> {
        None
    }

    /// Install a locally trusted protected mutable value without a network round trip.
    fn install_protected_mutable(&mut self, _item: MutableItem) -> bool {
        false
    }

    /// Read a protected mutable value from local server storage.
    fn protected_mutable(&self, _target: &Id) -> Option<MutableItem> {
        None
    }

    /// Open the inbound request gate only if local protected storage matches the trusted value.
    fn start_serving_if_protected_matches(&mut self, _item: &MutableItem) -> bool {
        false
    }

    /// Handle incoming requests.
    ///
    /// This function will block the main loop where the [Rpc]
    /// is running, thus it needs to be very fast and lightweight.
    fn handle_request(
        &mut self,
        rpc: &mut Rpc,
        from: SocketAddr,
        transaction_id: u16,
        request: &RequestSpecific,
    );
}

pub trait MutableItemValidator: std::fmt::Debug + Send + Sync {
    fn validate(&self, item: &MutableItem) -> bool;

    fn is_protected(&self, _item: &MutableItem) -> bool {
        false
    }

    fn is_protected_target(&self, _target: &Id) -> bool {
        false
    }
}

pub(crate) fn is_protected_mutable_request(
    request: &RequestSpecific,
    validator: &dyn MutableItemValidator,
) -> bool {
    let target = match &request.request_type {
        RequestTypeSpecific::GetValue(request) => &request.target,
        RequestTypeSpecific::Put(PutRequest {
            put_request_type: PutRequestSpecific::PutMutable(request),
            ..
        }) => &request.target,
        _ => return false,
    };
    validator.is_protected_target(target)
}

fn same_mutable_item(first: &MutableItem, second: &MutableItem) -> bool {
    first.target() == second.target()
        && first.seq() == second.seq()
        && first.key() == second.key()
        && first.value() == second.value()
        && first.signature() == second.signature()
        && first.salt() == second.salt()
}

#[derive(Debug)]
/// Default implementation of [Server] trait.
///
/// Supports [BEP0005](https://www.bittorrent.org/beps/bep_0005.html) and [BEP_0044](https://www.bittorrent.org/beps/bep_0044.html).
///
/// But it doesn't implement any rate-limiting or blocking.
pub struct DhtServer {
    tokens: Tokens,
    // server storage
    peers: PeersStore,

    immutable_values: LruCache<Id, Bytes>,
    mutable_values: Cache<Id, MutableItem>,
    protected_mutable_values: HashMap<Id, MutableItem>,
    mutable_item_validator: Option<Arc<dyn MutableItemValidator>>,
    request_gate: Option<Arc<AtomicBool>>,
}

impl Default for DhtServer {
    fn default() -> Self {
        DhtServer::new(&DhtServerSettings::default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct DhtServerSettings {
    /// The maximum info_hashes for which to store peers.
    ///
    /// Defaults to [MAX_INFO_HASHES]
    pub max_info_hashes: usize,
    /// The maximum peers to store per info_hash.
    ///
    /// Defaults to [MAX_PEERS]
    pub max_peers_per_info_hash: usize,
    /// Maximum number of immutable values to store.
    ///
    /// Defaults to [MAX_VALUES]
    pub max_immutable_values: usize,
    /// Maximum number of mutable values to store.
    ///
    /// Defaults to [MAX_VALUES]
    pub max_mutable_values: usize,
    /// Optional application-level validation applied before storing mutable values.
    pub mutable_item_validator: Option<Arc<dyn MutableItemValidator>>,
    /// Optional gate that must be enabled before inbound requests are handled.
    pub request_gate: Option<Arc<AtomicBool>>,
}

impl DhtServer {
    pub fn new(settings: &DhtServerSettings) -> Self {
        let tokens = Tokens::new();

        Self {
            tokens,
            peers: PeersStore::new(
                NonZeroUsize::new(settings.max_info_hashes).unwrap_or(
                    NonZeroUsize::new(MAX_INFO_HASHES).expect("MAX_PEERS is NonZeroUsize"),
                ),
                NonZeroUsize::new(settings.max_peers_per_info_hash)
                    .unwrap_or(NonZeroUsize::new(MAX_PEERS).expect("MAX_PEERS is NonZeroUsize")),
            ),

            immutable_values: LruCache::new(
                NonZeroUsize::new(settings.max_immutable_values)
                    .unwrap_or(NonZeroUsize::new(MAX_VALUES).expect("MAX_VALUES is NonZeroUsize")),
            ),
            mutable_values: Cache::builder()
                .max_capacity(
                    NonZeroUsize::new(settings.max_mutable_values)
                        .unwrap_or(NonZeroUsize::new(MAX_VALUES).expect("MAX_VALUES is NonZeroUsize"))
                        .get() as u64
                )
                .time_to_idle(Duration::from_secs(60 * 60 * 2))
                .build(),
            protected_mutable_values: HashMap::new(),
            mutable_item_validator: settings.mutable_item_validator.clone(),
            request_gate: settings.request_gate.clone(),
            // mutable_values: LruCache::new(
            //     NonZeroUsize::new(settings.max_mutable_values)
            //         .unwrap_or(NonZeroUsize::new(MAX_VALUES).expect("MAX_VALUES is NonZeroUsize")),
            // )
        }
    }

    // === Private Methods ===

    /// Handle get mutable request
    fn handle_get_mutable(
        &mut self,
        rpc: &mut Rpc,
        from: SocketAddr,
        transaction_id: u16,
        target: &Id,
        seq: &Option<i64>,
    ) {
        let item = self
            .protected_mutable_values
            .get(target)
            .cloned()
            .or_else(|| self.mutable_values.get(target));
        let item = if item.as_ref().is_some_and(|item| {
            self.mutable_item_validator
                .as_ref()
                .is_some_and(|validator| !validator.validate(item))
        }) {
            self.protected_mutable_values.remove(target);
            self.mutable_values.invalidate(target);
            None
        } else {
            item
        };
        rpc.response(
            from,
            transaction_id,
            match item {
                Some(item) => {
                    let no_more_recent_values = seq.map(|request_seq| item.seq() <= &request_seq);

                    match no_more_recent_values {
                        Some(true) => ResponseSpecific::NoMoreRecentValue(
                            NoMoreRecentValueResponseArguments {
                                responder_id: *rpc.id(),
                                token: self.tokens.generate_token(from).into(),
                                nodes: Some(rpc.routing_table().closest(target)),
                                seq: *item.seq(),
                            },
                        ),
                        _ => ResponseSpecific::GetMutable(GetMutableResponseArguments {
                            responder_id: *rpc.id(),
                            token: self.tokens.generate_token(from).into(),
                            nodes: Some(rpc.routing_table().closest(target)),
                            v: item.value().to_vec(),
                            k: item.key().to_vec(),
                            seq: *item.seq(),
                            sig: item.signature().to_vec(),
                        }),
                    }
                }
                None => ResponseSpecific::NoValues(NoValuesResponseArguments {
                    responder_id: *rpc.id(),
                    token: self.tokens.generate_token(from).into(),
                    nodes: Some(rpc.routing_table().closest(target)),
                }),
            },
        )
    }
}

impl Server for DhtServer {
    fn inbound_request_gate(&self) -> Option<Arc<AtomicBool>> {
        self.request_gate.clone()
    }

    fn gated_mutable_item_validator(&self) -> Option<Arc<dyn MutableItemValidator>> {
        self.mutable_item_validator.clone()
    }

    fn install_protected_mutable(&mut self, item: MutableItem) -> bool {
        let Some(validator) = &self.mutable_item_validator else {
            return false;
        };
        if !validator.is_protected(&item) || !validator.validate(&item) {
            return false;
        }
        if let Some(existing) = self.protected_mutable_values.get(item.target()) {
            if existing.seq() > item.seq() {
                return false;
            }
            if existing.seq() == item.seq() {
                return same_mutable_item(existing, &item);
            }
        }
        self.mutable_values.invalidate(item.target());
        self.protected_mutable_values.insert(*item.target(), item);
        true
    }

    fn protected_mutable(&self, target: &Id) -> Option<MutableItem> {
        self.protected_mutable_values.get(target).cloned()
    }

    fn start_serving_if_protected_matches(&mut self, item: &MutableItem) -> bool {
        let valid = self
            .mutable_item_validator
            .as_ref()
            .is_some_and(|validator| validator.is_protected(item) && validator.validate(item));
        let matches = valid
            && self
                .protected_mutable_values
                .get(item.target())
                .is_some_and(|stored| same_mutable_item(stored, item));
        if matches {
            if let Some(gate) = &self.request_gate {
                gate.store(true, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Handle incoming request.
    fn handle_request(
        &mut self,
        rpc: &mut Rpc,
        from: SocketAddr,
        transaction_id: u16,
        request: &RequestSpecific,
    ) {
        if self.request_gate.as_ref().is_some_and(|gate| {
            !gate.load(Ordering::Acquire)
                && !self
                    .mutable_item_validator
                    .as_deref()
                    .is_some_and(|validator| is_protected_mutable_request(request, validator))
        }) {
            return;
        }
        // Lazily rotate secrets before handling a request
        if self.tokens.should_update() {
            self.tokens.rotate()
        }

        let requester_id = request.requester_id;

        match &request.request_type {
            RequestTypeSpecific::Ping => {
                rpc.response(
                    from,
                    transaction_id,
                    ResponseSpecific::Ping(PingResponseArguments {
                        responder_id: *rpc.id(),
                    }),
                );
            }
            RequestTypeSpecific::FindNode(FindNodeRequestArguments { target, .. }) => {
                rpc.response(
                    from,
                    transaction_id,
                    ResponseSpecific::FindNode(FindNodeResponseArguments {
                        responder_id: *rpc.id(),
                        nodes: rpc.routing_table().closest(target),
                    }),
                );
            }
            RequestTypeSpecific::GetPeers(GetPeersRequestArguments { info_hash, .. }) => {
                rpc.response(
                    from,
                    transaction_id,
                    match self.peers.get_random_peers(info_hash) {
                        Some(peers) => ResponseSpecific::GetPeers(GetPeersResponseArguments {
                            responder_id: *rpc.id(),
                            token: self.tokens.generate_token(from).into(),
                            nodes: Some(rpc.routing_table().closest(info_hash)),
                            values: peers,
                        }),
                        None => ResponseSpecific::NoValues(NoValuesResponseArguments {
                            responder_id: *rpc.id(),
                            token: self.tokens.generate_token(from).into(),
                            nodes: Some(rpc.routing_table().closest(info_hash)),
                        }),
                    },
                );
            }
            RequestTypeSpecific::GetValue(GetValueRequestArguments { target, seq, .. }) => {
                if seq.is_some() {
                    return self.handle_get_mutable(rpc, from, transaction_id, target, seq);
                }

                if let Some(v) = self.immutable_values.get(target) {
                    rpc.response(
                        from,
                        transaction_id,
                        ResponseSpecific::GetImmutable(GetImmutableResponseArguments {
                            responder_id: *rpc.id(),
                            token: self.tokens.generate_token(from).into(),
                            nodes: Some(rpc.routing_table().closest(target)),
                            v: v.to_vec(),
                        }),
                    )
                } else {
                    self.handle_get_mutable(rpc, from, transaction_id, target, seq);
                };
            }
            RequestTypeSpecific::Put(PutRequest {
                token,
                put_request_type,
            }) => match put_request_type {
                PutRequestSpecific::AnnouncePeer(AnnouncePeerRequestArguments {
                    info_hash,
                    port,
                    implied_port,
                    ..
                }) => {
                    if !self.tokens.validate(from, token) {
                        rpc.error(
                            from,
                            transaction_id,
                            ErrorSpecific {
                                code: 203,
                                description: "Bad token".to_string(),
                            },
                        );
                        debug!(
                            ?info_hash,
                            ?requester_id,
                            ?from,
                            ?token,
                            request_type = "announce_peer",
                            "Invalid token"
                        );
                        return;
                    }

                    let peer = match implied_port {
                        Some(true) => from,
                        _ => SocketAddr::new(from.ip(), *port),
                    };

                    self.peers
                        .add_peer(*info_hash, (&request.requester_id, peer));

                    rpc.response(
                        from,
                        transaction_id,
                        ResponseSpecific::Ping(PingResponseArguments {
                            responder_id: *rpc.id(),
                        }),
                    );
                }
                PutRequestSpecific::PutImmutable(PutImmutableRequestArguments {
                    v,
                    target,
                    ..
                }) => {
                    if !self.tokens.validate(from, token) {
                        rpc.error(
                            from,
                            transaction_id,
                            ErrorSpecific {
                                code: 203,
                                description: "Bad token".to_string(),
                            },
                        );
                        debug!(
                            ?target,
                            ?requester_id,
                            ?from,
                            ?token,
                            request_type = "put_immutable",
                            "Invalid token"
                        );
                        return;
                    }

                    if v.len() > 1000 {
                        rpc.error(
                            from,
                            transaction_id,
                            ErrorSpecific {
                                code: 205,
                                description: "Message (v field) too big.".to_string(),
                            },
                        );
                        debug!(?target, ?requester_id, ?from, size = ?v.len(), "Message (v field) too big.");
                        return;
                    }
                    if !validate_immutable(v, target) {
                        rpc.error(
                            from,
                            transaction_id,
                            ErrorSpecific {
                                code: 203,
                                description: "Target doesn't match the sha1 hash of v field"
                                    .to_string(),
                            },
                        );
                        debug!(?target, ?requester_id, ?from, v = ?v, "Target doesn't match the sha1 hash of v field.");
                        return;
                    }

                    self.immutable_values.put(*target, v.to_owned().into());

                    rpc.response(
                        from,
                        transaction_id,
                        ResponseSpecific::Ping(PingResponseArguments {
                            responder_id: *rpc.id(),
                        }),
                    );
                }
                PutRequestSpecific::PutMutable(PutMutableRequestArguments {
                    target,
                    v,
                    k,
                    seq,
                    sig,
                    salt,
                    cas,
                    ..
                }) => {
                    if !self.tokens.validate(from, token) {
                        rpc.error(
                            from,
                            transaction_id,
                            ErrorSpecific {
                                code: 203,
                                description: "Bad token".to_string(),
                            },
                        );
                        debug!(
                            ?target,
                            ?requester_id,
                            ?from,
                            ?token,
                            request_type = "put_mutable",
                            "Invalid token"
                        );
                        return;
                    }
                    if v.len() > 1000 {
                        rpc.error(
                            from,
                            transaction_id,
                            ErrorSpecific {
                                code: 205,
                                description: "Message (v field) too big.".to_string(),
                            },
                        );
                        return;
                    }
                    if let Some(salt) = salt {
                        if salt.len() > 64 {
                            rpc.error(
                                from,
                                transaction_id,
                                ErrorSpecific {
                                    code: 207,
                                    description: "salt (salt field) too big.".to_string(),
                                },
                            );
                            return;
                        }
                    }
                    if let Some(previous) = self
                        .protected_mutable_values
                        .get(target)
                        .cloned()
                        .or_else(|| self.mutable_values.get(target))
                    {
                        if let Some(cas) = cas {
                            if previous.seq() != cas {
                                rpc.error(
                                    from,
                                    transaction_id,
                                    ErrorSpecific {
                                        code: 301,
                                        description: "CAS mismatched, re-read value and try again."
                                            .to_string(),
                                    },
                                );
                                debug!(
                                    ?target,
                                    ?requester_id,
                                    ?from,
                                    "CAS mismatched, re-read value and try again."
                                );

                                return;
                            }
                        };

                        if seq <= previous.seq() {
                            rpc.error(
                                from,
                                transaction_id,
                                ErrorSpecific {
                                    code: 302,
                                    description: "Sequence number less than or equal to current.".to_string(),
                                },
                            );
                            debug!(
                                ?target,
                                ?requester_id,
                                ?from,
                                incoming_sequence = *seq,
                                stored_sequence = *previous.seq(),
                                "Mutable PUT rejected: sequence number less than or equal to current"
                            );

                            return;
                        }
                    }

                    match MutableItem::from_dht_message(
                        target,
                        k,
                        v.to_owned().into(),
                        seq,
                        sig,
                        salt.to_owned().map(|v| v.into()),
                        cas,
                    ) {
                        Ok(item) => {
                            if self
                                .mutable_item_validator
                                .as_ref()
                                .is_some_and(|validator| !validator.validate(&item))
                            {
                                rpc.error(
                                    from,
                                    transaction_id,
                                    ErrorSpecific {
                                        code: 206,
                                        description:
                                            "Mutable value rejected by application validator"
                                                .to_string(),
                                    },
                                );
                                debug!(
                                    ?target,
                                    ?requester_id,
                                    ?from,
                                    "Mutable value rejected by application validator"
                                );
                                return;
                            }
                            if self
                                .mutable_item_validator
                                .as_ref()
                                .is_some_and(|validator| validator.is_protected(&item))
                            {
                                self.mutable_values.invalidate(target);
                                self.protected_mutable_values.insert(*target, item);
                            } else {
                                self.mutable_values.insert(*target, item);
                            }

                            rpc.response(
                                from,
                                transaction_id,
                                ResponseSpecific::Ping(PingResponseArguments {
                                    responder_id: *rpc.id(),
                                }),
                            );
                        }
                        Err(error) => {
                            rpc.error(
                                from,
                                transaction_id,
                                ErrorSpecific {
                                    code: 206,
                                    description: "Invalid signature".to_string(),
                                },
                            );

                            debug!(?target, ?requester_id, ?from, ?error, "Invalid signature");
                        }
                    }
                }
            },
        }
    }
}
