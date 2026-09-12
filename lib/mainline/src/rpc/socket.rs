//! DTLS socket layer managing incoming/outgoing requests and responses.

use lru::LruCache;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tracing::{debug, trace};

use crate::common::{ErrorSpecific, Message, MessageType, RequestSpecific, ResponseSpecific};

use super::transport::{DtlsTransport, TransportEvent};
use crate::error::SocketAddrResult;
use crate::security::{CertificateId, RevocationSnapshot, RevocationStore};
use crate::server::{is_protected_mutable_request, MutableItemValidator};
use crate::{dht::DhtSettings, Error, Result};

const VERSION: [u8; 4] = [82, 83, 0, 1]; // "RS" version 01
pub const DEFAULT_PORT: u16 = 6881;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_millis(2000); // 2 seconds
pub const READ_TIMEOUT: Duration = Duration::from_millis(10);
const MAX_AUTHENTICATED_PEERS: usize = 1024;

/// A DTLS transport wrapper that formats and correlates DHT requests and responses.
#[derive(Debug)]
pub struct KrpcSocket {
    next_tid: u16,
    transport: DtlsTransport,
    read_only: bool,
    request_gate: Option<Arc<AtomicBool>>,
    gated_mutable_item_validator: Option<Arc<dyn MutableItemValidator>>,
    request_timeout: Duration,
    /// We don't need a HashMap, since we know the capacity is `65536` requests.
    /// Requests are also ordered by their transaction_id and thus sent_at, so lookup is fast.
    inflight_requests: Vec<InflightRequest>,
    reply_sessions: HashMap<(SocketAddr, u16), ReplySession>,
    authenticated_peers: LruCache<SocketAddr, AuthenticatedPeer>,
    revocations: RevocationStore,
    revoked_addresses: Vec<SocketAddr>,
}

#[derive(Debug)]
pub struct InflightRequest {
    recovery: bool,
    tid: u16,
    to: SocketAddr,
    queued_at: Instant,
    sent_at: Option<Instant>,
}

#[derive(Debug)]
struct ReplySession {
    recovery: bool,
    id: u64,
    received_at: Instant,
}

#[derive(Debug)]
struct AuthenticatedPeer {
    certificate: CertificateId,
}

impl KrpcSocket {
    pub fn new(settings: &DhtSettings) -> Result<Self> {
        let revocations = settings
            .dtls
            .as_ref()
            .ok_or(Error::MissingDtlsConfig)?
            .revocations()
            .clone();
        let request_gate = settings
            .server
            .as_ref()
            .and_then(|server| server.inbound_request_gate());
        let gated_mutable_item_validator = settings
            .server
            .as_ref()
            .and_then(|server| server.gated_mutable_item_validator());
        let transport = DtlsTransport::new(settings, request_gate.clone())?;

        Ok(Self {
            transport,
            next_tid: 0,
            read_only: settings.server.is_none(),
            request_gate,
            gated_mutable_item_validator,
            request_timeout: settings.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT),
            inflight_requests: Vec::with_capacity(u16::MAX as usize),
            reply_sessions: HashMap::new(),
            authenticated_peers: LruCache::new(
                NonZeroUsize::new(MAX_AUTHENTICATED_PEERS)
                    .expect("MAX_AUTHENTICATED_PEERS is non-zero"),
            ),
            revocations,
            revoked_addresses: Vec::new(),
        })
    }

    // === Options ===

    /// Returns the address the server is listening to.
    #[inline]
    pub fn local_addr(&self) -> SocketAddrResult {
        Ok(self.transport.local_addr())
    }

    // === Public Methods ===

    pub(super) fn revocations(&self) -> RevocationStore {
        self.revocations.clone()
    }

    pub(super) fn is_recovery_request(&self, request: &RequestSpecific) -> bool {
        self.gated_mutable_item_validator
            .as_deref()
            .is_some_and(|validator| is_protected_mutable_request(request, validator))
    }

    /// Returns true if this message's transaction_id is still inflight
    pub fn inflight(&self, transaction_id: &u16) -> bool {
        self.inflight_requests
            .iter()
            .any(|request| request.tid == *transaction_id)
    }

    /// Send a request to the given address and return the transaction_id
    pub fn request(&mut self, address: SocketAddr, request: RequestSpecific) -> u16 {
        let recovery = self
            .gated_mutable_item_validator
            .as_deref()
            .is_some_and(|validator| is_protected_mutable_request(&request, validator));
        let message = self.request_message(request);
        if self.revocations.security_failed() && !recovery {
            return message.transaction_id;
        }

        self.inflight_requests.push(InflightRequest {
            recovery,
            tid: message.transaction_id,
            to: address,
            queued_at: Instant::now(),
            sent_at: None,
        });

        let tid = message.transaction_id;
        let _ = self.send_request(address, message).map_err(|e| {
            debug!(?e, "Error sending request message");
        });

        tid
    }

    /// Send a response to the given address.
    pub fn response(
        &mut self,
        address: SocketAddr,
        transaction_id: u16,
        response: ResponseSpecific,
    ) {
        let message =
            self.response_message(MessageType::Response(response), address, transaction_id);
        let Some(session) = self.reply_sessions.remove(&(address, transaction_id)) else {
            debug!(
                ?address,
                transaction_id, "DTLS reply session is unavailable"
            );
            return;
        };
        let _ = self
            .send_response(address, session.id, message, session.recovery)
            .map_err(|e| {
                debug!(?e, "Error sending response message");
            });
    }

    /// Send an error to the given address.
    pub fn error(&mut self, address: SocketAddr, transaction_id: u16, error: ErrorSpecific) {
        let message = self.response_message(MessageType::Error(error), address, transaction_id);
        let Some(session) = self.reply_sessions.remove(&(address, transaction_id)) else {
            debug!(
                ?address,
                transaction_id, "DTLS reply session is unavailable"
            );
            return;
        };
        let _ = self
            .send_response(address, session.id, message, session.recovery)
            .map_err(|e| {
                debug!(?e, "Error sending error message");
            });
    }

    /// Receives a single krpc message on the socket.
    /// On success, returns the dht message and the origin.
    pub fn recv_from(&mut self) -> Option<(Message, SocketAddr)> {
        if self.revocations.security_failed() {
            self.inflight_requests.retain(|request| request.recovery);
        }
        let request_timeout = self.request_timeout;
        let handshake_timeout = self.transport.handshake_timeout();
        self.inflight_requests
            .retain(|request| match request.sent_at {
                Some(sent_at) => sent_at.elapsed() <= request_timeout,
                None => request.queued_at.elapsed() <= handshake_timeout,
            });
        self.reply_sessions
            .retain(|_, session| session.received_at.elapsed() <= request_timeout);

        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline.saturating_duration_since(now);
            let event = self.transport.recv_timeout(remaining)?;
            match event {
                TransportEvent::Sent {
                    transaction_id,
                    address,
                } => {
                    if let Some(request) = self.inflight_requests.iter_mut().find(|request| {
                        request.tid == transaction_id && compare_socket_addr(&request.to, &address)
                    }) {
                        request.sent_at = Some(Instant::now());
                    }
                }
                TransportEvent::SendFailed {
                    transaction_id,
                    address,
                } => {
                    self.inflight_requests.retain(|request| {
                        request.tid != transaction_id || !compare_socket_addr(&request.to, &address)
                    });
                }
                TransportEvent::PeerRevoked {
                    session_id,
                    address,
                } => {
                    self.revoked_addresses.push(address);
                    let mut session_peers = Vec::new();
                    self.reply_sessions.retain(|(peer, _), session| {
                        if session.id == session_id {
                            session_peers.push(*peer);
                            false
                        } else {
                            true
                        }
                    });
                    self.revoked_addresses.extend(session_peers);
                    self.purge_revoked_peers();
                }
                TransportEvent::SessionClosed { session_id } => {
                    self.reply_sessions
                        .retain(|_, session| session.id != session_id);
                }
                TransportEvent::Datagram {
                    bytes,
                    address: from,
                    session_id,
                    accepted,
                    received_while_gated,
                    certificate,
                    active,
                    invalidated,
                    pending_events,
                } => {
                    let _ = pending_events.fetch_update(
                        Ordering::AcqRel,
                        Ordering::Acquire,
                        |pending| pending.checked_sub(1),
                    );
                    if self.revocations.is_revoked(&certificate) {
                        self.revoked_addresses.push(from);
                        continue;
                    }
                    if invalidated.load(Ordering::Acquire) {
                        continue;
                    }
                    match Message::from_bytes(&bytes) {
                        Ok(mut message) => {
                            let is_request =
                                matches!(&message.message_type, MessageType::Request(_));
                            if !is_request
                                && self.revocations.security_failed()
                                && !self.inflight_requests.iter().any(|request|
                                    request.tid == message.transaction_id && request.recovery
                                )
                            {
                                continue;
                            }
                            if let MessageType::Request(request) = &message.message_type {
                                if !active.load(Ordering::Acquire) || self.read_only {
                                    continue;
                                }
                                let gate_is_closed = self
                                    .request_gate
                                    .as_ref()
                                    .is_some_and(|gate| !gate.load(Ordering::Acquire));
                                if received_while_gated && !gate_is_closed {
                                    continue;
                                }
                                let gated = received_while_gated
                                    || gate_is_closed
                                    || self.revocations.security_failed();
                                if gated
                                    && !self.gated_mutable_item_validator.as_deref().is_some_and(
                                        |validator| {
                                            is_protected_mutable_request(request, validator)
                                        },
                                    )
                                {
                                    continue;
                                }
                                if gated {
                                    message.read_only = true;
                                }
                            }
                            let from = if accepted && is_request {
                                message
                                    .requester_ip
                                    .map(|advertised| SocketAddr::new(from.ip(), advertised.port()))
                                    .unwrap_or(from)
                            } else {
                                from
                            };
                            self.authenticated_peers
                                .put(from, AuthenticatedPeer { certificate });
                            // Parsed correctly.
                            match message.message_type {
                                MessageType::Request(ref request) => {
                                    self.reply_sessions.insert(
                                        (from, message.transaction_id),
                                        ReplySession {
                                            recovery: self
                                                .gated_mutable_item_validator
                                                .as_deref()
                                                .is_some_and(|validator|
                                                    is_protected_mutable_request(request, validator)
                                                ),
                                            id: session_id,
                                            received_at: Instant::now(),
                                        },
                                    );
                                    return Some((message, from));
                                }
                                // Response or an error to an inflight request.
                                _ => {
                                    match self
                                        .inflight_requests
                                        .iter()
                                        .position(|request| request.tid == message.transaction_id)
                                    {
                                        Some(index) => {
                                            match self.inflight_requests.get(index) {
                                                Some(inflight_request) => {
                                                    if compare_socket_addr(
                                                        &inflight_request.to,
                                                        &from,
                                                    ) {
                                                        // Confirm that it is a response we actually sent.
                                                        self.inflight_requests.remove(index);

                                                        return Some((message, from));
                                                    }
                                                    trace!(
                                                        ?message,
                                                        "Response from the wrong address"
                                                    );
                                                }
                                                _ => panic!("should not return None"),
                                            };
                                        }
                                        None => {
                                            trace!(?message, "Unexpected response id");
                                        }
                                    };
                                }
                            }
                        }
                        Err(error) => {
                            trace!(?error, ?bytes, "Received invalid message");
                        }
                    }
                }
            }
        }
    }

    // === Private Methods ===

    /// Increments self.next_tid and returns the previous value.
    fn tid(&mut self) -> u16 {
        // We don't bother much with reusing freed transaction ids,
        // since the timeout is so short we are unlikely to run out
        // of 65535 ids in 2 seconds.
        let tid = self.next_tid;
        self.next_tid = self.next_tid.wrapping_add(1);
        tid
    }

    fn is_read_only(&self) -> bool {
        self.read_only
            || self
                .request_gate
                .as_ref()
                .is_some_and(|gate| !gate.load(Ordering::Acquire))
    }

    /// Set transactin_id, version and read_only
    fn request_message(&mut self, message: RequestSpecific) -> Message {
        let transaction_id = self.tid();

        Message {
            transaction_id,
            message_type: MessageType::Request(message),
            version: Some(VERSION.into()),
            read_only: self.is_read_only(),
            requester_ip: Some(SocketAddr::new(
                Ipv4Addr::UNSPECIFIED.into(),
                self.transport.local_addr().port(),
            )),
        }
    }

    /// Same as request_message but with request transaction_id and the requester_ip.
    fn response_message(
        &mut self,
        message: MessageType,
        requester_ip: SocketAddr,
        request_tid: u16,
    ) -> Message {
        Message {
            transaction_id: request_tid,
            message_type: message,
            version: Some(VERSION.into()),
            read_only: self.is_read_only(),
            // BEP0042 Only relevant in responses.
            requester_ip: Some(requester_ip),
        }
    }

    /// Send a raw dht message
    fn send_request(&mut self, address: SocketAddr, message: Message) -> Result<()> {
        trace!(?message, "Sending a message");
        let transaction_id = message.transaction_id;
        let recovery = matches!(
            &message.message_type,
            MessageType::Request(request)
                if self.gated_mutable_item_validator.as_deref()
                    .is_some_and(|validator| is_protected_mutable_request(request, validator))
        );
        self.transport
            .send_request(address, transaction_id, message.to_bytes()?, recovery)
    }

    fn send_response(
        &mut self,
        address: SocketAddr,
        session_id: u64,
        message: Message,
        recovery: bool,
    ) -> Result<()> {
        trace!(?message, "Sending a message");
        self.transport
            .send_response(address, session_id, message.to_bytes()?, recovery)
    }

    pub fn replace_revocations(&mut self, snapshot: RevocationSnapshot) -> bool {
        let replaced = self.revocations.replace(snapshot.clone());
        let applied = replaced || self.revocations.snapshot() == snapshot;
        if applied {
            self.purge_revoked_peers();
            self.transport.evict_revoked();
        }
        applied
    }

    pub(crate) fn revocation_sequence(&self) -> i64 {
        self.revocations.snapshot().sequence
    }

    pub(crate) fn reset_sessions(&mut self) {
        self.inflight_requests.clear();
        self.reply_sessions.clear();
        self.authenticated_peers.clear();
        self.transport.reset_sessions();
    }

    fn purge_revoked_peers(&mut self) {
        let revoked = self
            .authenticated_peers
            .iter()
            .filter_map(|(address, peer)| {
                self.revocations
                    .is_revoked(&peer.certificate)
                    .then_some(*address)
            })
            .collect::<Vec<_>>();
        for address in revoked {
            self.authenticated_peers.pop(&address);
            self.revoked_addresses.push(address);
        }
    }

    pub fn take_revoked_addresses(&mut self) -> Vec<SocketAddr> {
        std::mem::take(&mut self.revoked_addresses)
    }
}

// Same as SocketAddr::eq but ingores the ip if it is unspecified for testing reasons.
fn compare_socket_addr(a: &SocketAddr, b: &SocketAddr) -> bool {
    if a.port() != b.port() {
        return false;
    }

    if a.ip().is_unspecified() {
        return true;
    }

    a.ip() == b.ip()
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use crate::{
        common::{Id, PingResponseArguments, RequestTypeSpecific},
        security::{test_dtls_configs, DtlsConfig},
        server::{DhtServer, DhtServerSettings},
    };

    use super::*;

    fn new_socket(config: DtlsConfig, server: bool) -> KrpcSocket {
        KrpcSocket::new(&DhtSettings {
            server: server.then(|| Box::<DhtServer>::default() as Box<dyn crate::server::Server>),
            bootstrap: None,
            request_timeout: Some(Duration::from_secs(2)),
            port: Some(0),
            dtls: Some(config),
        })
        .unwrap()
    }

    fn new_gated_socket(config: DtlsConfig, gate: Arc<AtomicBool>) -> KrpcSocket {
        let server = DhtServer::new(&DhtServerSettings {
            request_gate: Some(gate),
            ..Default::default()
        });
        KrpcSocket::new(&DhtSettings {
            server: Some(Box::new(server)),
            bootstrap: None,
            request_timeout: Some(Duration::from_secs(2)),
            port: Some(0),
            dtls: Some(config),
        })
        .unwrap()
    }

    fn socket_pair(first_server: bool, second_server: bool) -> (KrpcSocket, KrpcSocket) {
        let mut configs = test_dtls_configs(2).unwrap();
        let second = new_socket(configs.pop().unwrap(), second_server);
        let first = new_socket(configs.pop().unwrap(), first_server);
        (first, second)
    }

    fn loopback(address: SocketAddr) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], address.port()))
    }

    fn receive(socket: &mut KrpcSocket) -> (Message, SocketAddr) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(message) = socket.recv_from() {
                return message;
            }
        }
        panic!("timed out waiting for a DTLS message");
    }

    fn assert_no_message(socket: &mut KrpcSocket) {
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            assert!(socket.recv_from().is_none());
        }
    }

    fn assert_request_fails(
        requester: &mut KrpcSocket,
        receiver: &mut KrpcSocket,
        transaction_id: u16,
    ) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while requester.inflight(&transaction_id) && Instant::now() < deadline {
            assert!(receiver.recv_from().is_none());
            let _ = requester.recv_from();
        }
        assert!(!requester.inflight(&transaction_id));
    }

    #[test]
    fn tid() {
        let mut configs = test_dtls_configs(1).unwrap();
        let mut socket = new_socket(configs.pop().unwrap(), false);

        assert_eq!(socket.tid(), 0);
        assert_eq!(socket.tid(), 1);
        assert_eq!(socket.tid(), 2);

        socket.next_tid = u16::MAX;

        assert_eq!(socket.tid(), 65535);
        assert_eq!(socket.tid(), 0);
    }

    #[test]
    fn recv_request() {
        let (mut server, mut client) = socket_pair(true, false);
        let server_address = loopback(server.local_addr().unwrap());
        client.next_tid = 120;
        let client_address = client.local_addr().unwrap();
        let request = RequestSpecific {
            requester_id: Id::random(),
            request_type: RequestTypeSpecific::Ping,
        };
        let expected_request = request.clone();
        client.request(server_address, request);

        let (message, from) = receive(&mut server);
        assert_eq!(from.port(), client_address.port());
        assert_eq!(message.transaction_id, 120);
        assert!(message.read_only, "Read-only should be true");
        assert_eq!(
            message.version,
            Some(VERSION.to_vec()),
            "Version should be 'RS'"
        );
        assert_eq!(message.message_type, MessageType::Request(expected_request));
    }

    #[test]
    fn recv_response() {
        let (mut server, mut client) = socket_pair(true, false);
        let server_address = loopback(server.local_addr().unwrap());
        client.next_tid = 8;
        client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (request, client_address) = receive(&mut server);
        let response = ResponseSpecific::Ping(PingResponseArguments {
            responder_id: Id::random(),
        });
        let expected_response = response.clone();
        server.response(client_address, request.transaction_id, response);

        let (message, from) = receive(&mut client);
        assert_eq!(from.port(), server_address.port());
        assert_eq!(message.transaction_id, 8);
        assert!(!message.read_only, "Server should not be read-only");
        assert_eq!(
            message.version,
            Some(VERSION.to_vec()),
            "Version should be 'RS'"
        );
        assert_eq!(
            message.message_type,
            MessageType::Response(expected_response)
        );
    }

    #[test]
    fn ignore_unexcpected_response() {
        let (mut server, mut client) = socket_pair(true, false);
        let server_address = loopback(server.local_addr().unwrap());
        client.next_tid = 120;
        client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (request, client_address) = receive(&mut server);
        let response = ResponseSpecific::Ping(PingResponseArguments {
            responder_id: Id::random(),
        });
        server.response(client_address, request.transaction_id + 1, response);

        assert_no_message(&mut client);
    }

    #[test]
    fn ignore_response_from_wrong_address() {
        let (mut requester, mut responder) = socket_pair(false, true);
        let responder_address = loopback(responder.local_addr().unwrap());
        let tid = requester.request(
            responder_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        requester.inflight_requests[0].to =
            SocketAddr::from(([127, 0, 0, 1], responder_address.port().wrapping_add(1)));
        let (request, requester_address) = receive(&mut responder);
        let response = ResponseSpecific::Ping(PingResponseArguments {
            responder_id: Id::random(),
        });
        assert_eq!(request.transaction_id, tid);
        responder.response(requester_address, tid, response);

        assert_no_message(&mut requester);
    }

    #[test]
    fn ignore_request_in_read_only() {
        let (mut server, mut client) = socket_pair(false, false);
        let server_address = loopback(server.local_addr().unwrap());
        client.next_tid = 120;
        let request = RequestSpecific {
            requester_id: Id::random(),
            request_type: RequestTypeSpecific::Ping,
        };
        client.request(server_address, request);

        assert_no_message(&mut server);
    }

    #[test]
    fn request_gate_blocks_inbound_state_until_opened() {
        let mut configs = test_dtls_configs(2).unwrap();
        let gate = Arc::new(AtomicBool::new(true));
        let mut server = new_gated_socket(configs.pop().unwrap(), Arc::clone(&gate));
        let client_config = configs.pop().unwrap();
        let mut client = new_socket(client_config.clone(), false);
        let server_address = loopback(server.local_addr().unwrap());
        let warmup = client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (_, client_address) = receive(&mut server);
        server.response(
            client_address,
            warmup,
            ResponseSpecific::Ping(PingResponseArguments {
                responder_id: Id::random(),
            }),
        );
        let _ = receive(&mut client);
        server.authenticated_peers.clear();
        server.reply_sessions.clear();

        gate.store(false, Ordering::Release);
        client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while server.transport.pending_datagrams() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(server.transport.pending_datagrams() > 0);

        assert!(server.authenticated_peers.is_empty());
        assert!(server.reply_sessions.is_empty());

        gate.store(true, Ordering::Release);
        drop(client);
        let mut client = new_socket(client_config, false);
        let transaction_id = client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (message, _) = receive(&mut server);
        assert_eq!(message.transaction_id, transaction_id);
        assert_eq!(server.authenticated_peers.len(), 1);
        assert_eq!(server.reply_sessions.len(), 1);
    }

    #[test]
    fn request_gate_allows_outbound_synchronization() {
        let mut configs = test_dtls_configs(2).unwrap();
        let gate = Arc::new(AtomicBool::new(false));
        let mut gated = new_gated_socket(configs.pop().unwrap(), Arc::clone(&gate));
        let mut server = new_socket(configs.pop().unwrap(), true);
        let server_address = loopback(server.local_addr().unwrap());
        let transaction_id = gated.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );

        let (request, gated_address) = receive(&mut server);
        assert!(request.read_only);
        server.response(
            gated_address,
            transaction_id,
            ResponseSpecific::Ping(PingResponseArguments {
                responder_id: Id::random(),
            }),
        );
        let (response, _) = receive(&mut gated);
        assert_eq!(response.transaction_id, transaction_id);
        assert!(!gated.inflight(&transaction_id));

        gate.store(true, Ordering::Release);
        gated.reset_sessions();
        let transaction_id = gated.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (request, _) = receive(&mut server);
        assert_eq!(request.transaction_id, transaction_id);
        assert!(!request.read_only);
    }

    #[test]
    fn rejects_plaintext_krpc() {
        let mut configs = test_dtls_configs(1).unwrap();
        let mut server = new_socket(configs.pop().unwrap(), true);
        let server_address = loopback(server.local_addr().unwrap());
        let raw_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let request = Message {
            transaction_id: 1,
            message_type: MessageType::Request(RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            }),
            version: Some(VERSION.into()),
            read_only: true,
            requester_ip: None,
        };

        raw_socket
            .send_to(&request.to_bytes().unwrap(), server_address)
            .unwrap();

        assert_no_message(&mut server);
    }

    #[test]
    fn fatal_security_failure_discards_inflight_and_blocks_background_requests() {
        let mut configs = test_dtls_configs(2).unwrap();
        let config = configs.pop().unwrap();
        let revocations = config.revocations().clone();
        let mut client = new_socket(config, false);
        let mut server = new_socket(configs.pop().unwrap(), true);
        let address = loopback(server.local_addr().unwrap());
        let tid = client.request(
            address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (_, from) = receive(&mut server);
        revocations.fail_security();
        server.response(
            from,
            tid,
            ResponseSpecific::Ping(PingResponseArguments {
                responder_id: Id::random(),
            }),
        );
        for _ in 0..20 {
            assert!(client.recv_from().is_none());
        }
        assert!(!client.inflight(&tid));
        let tid = client.request(
            address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        assert!(!client.inflight(&tid));
        assert!(server.recv_from().is_none());
    }

    #[test]
    fn accepts_dtls_after_repeated_invalid_handshakes() {
        let mut configs = test_dtls_configs(2).unwrap();
        let client_config = configs.pop().unwrap();
        let server_config = configs
            .pop()
            .unwrap()
            .with_handshake_timeout(Duration::from_secs(2));
        let mut server = new_socket(server_config, true);
        let mut client = new_socket(client_config, false);
        let server_address = loopback(server.local_addr().unwrap());
        let mut malformed_client_hello = vec![0; 67];
        malformed_client_hello[0] = 22;
        malformed_client_hello[1] = 0xfe;
        malformed_client_hello[2] = 0xfd;
        malformed_client_hello[12] = 54;
        malformed_client_hello[13] = 1;
        malformed_client_hello[16] = 42;
        malformed_client_hello[24] = 42;
        malformed_client_hello[25] = 0xfe;
        malformed_client_hello[26] = 0xfd;
        malformed_client_hello[62] = 2;
        malformed_client_hello[63] = 0xc0;
        malformed_client_hello[64] = 0x2b;
        malformed_client_hello[65] = 1;
        for _ in 0..(super::super::transport::ACCEPT_WORKERS * 2) {
            let raw_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            raw_socket
                .send_to(&malformed_client_hello, server_address)
                .unwrap();
        }
        std::thread::sleep(Duration::from_millis(2500));

        let transaction_id = client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (request, _) = receive(&mut server);
        assert_eq!(request.transaction_id, transaction_id);
    }

    #[test]
    fn rejects_revoked_certificate() {
        let mut configs = test_dtls_configs(2).unwrap();
        let client_config = configs
            .pop()
            .unwrap()
            .with_handshake_timeout(Duration::from_secs(1));
        let client_id = client_config.local_certificate_id().unwrap();
        let server_config = configs.pop().unwrap();
        assert!(server_config.revocations().replace(RevocationSnapshot {
            sequence: 1,
            issuer_spki_sha256: client_id.issuer_spki_sha256,
            revoked_serials: HashSet::from([client_id.serial]),
        }));
        let mut server = new_socket(server_config, true);
        let mut client = new_socket(client_config, false);
        let transaction_id = client.request(
            loopback(server.local_addr().unwrap()),
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );

        assert_request_fails(&mut client, &mut server, transaction_id);
    }

    #[test]
    fn rejects_certificate_from_untrusted_ca() {
        let mut server_configs = test_dtls_configs(1).unwrap();
        let mut client_configs = test_dtls_configs(1).unwrap();
        let mut server = new_socket(server_configs.pop().unwrap(), true);
        let mut client = new_socket(
            client_configs
                .pop()
                .unwrap()
                .with_handshake_timeout(Duration::from_secs(1)),
            false,
        );
        let transaction_id = client.request(
            loopback(server.local_addr().unwrap()),
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );

        assert_request_fails(&mut client, &mut server, transaction_id);
    }

    #[test]
    fn closes_session_when_certificate_is_revoked() {
        let mut configs = test_dtls_configs(2).unwrap();
        let client_config = configs.pop().unwrap();
        let client_id = client_config.local_certificate_id().unwrap();
        let server_config = configs.pop().unwrap();
        let mut server = new_socket(server_config, true);
        let mut client = new_socket(client_config, false);
        let server_address = loopback(server.local_addr().unwrap());
        let transaction_id = client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        let (_, client_address) = receive(&mut server);
        server.response(
            client_address,
            transaction_id,
            ResponseSpecific::Ping(PingResponseArguments {
                responder_id: Id::random(),
            }),
        );
        let _ = receive(&mut client);

        assert!(server.replace_revocations(RevocationSnapshot {
            sequence: 1,
            issuer_spki_sha256: client_id.issuer_spki_sha256,
            revoked_serials: HashSet::from([client_id.serial]),
        }));
        assert_no_message(&mut server);
        assert!(server.take_revoked_addresses().contains(&client_address));

        let transaction_id = client.request(
            server_address,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::Ping,
            },
        );
        assert_request_fails(&mut client, &mut server, transaction_id);
    }
}
