use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use async_trait::async_trait;
use dtls::conn::DTLSConn;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex};
use webrtc_util::conn::Listener;
use webrtc_util::{Conn, Error as UtilError};

use crate::dht::DhtSettings;
use crate::security::{CertificateId, DtlsConfig, RevocationStore};
use crate::{Error, Result};

const COMMAND_QUEUE_SIZE: usize = 1024;
const SESSION_QUEUE_SIZE: usize = 128;
const EVENT_QUEUE_SIZE: usize = 1024;
const KRPC_DATAGRAM_SIZE: usize = 4096;
pub(super) const ACCEPT_WORKERS: usize = 8;
const ACCEPT_BACKLOG: usize = 128;
const INBOUND_PACKET_QUEUE_SIZE: usize = 8;
const UDP_RECEIVE_MTU: usize = 8192;
const PRE_AUTH_MAX_PACKETS: usize = 256;
const PRE_AUTH_MAX_BYTES: usize = 256 * 1024;
const POST_AUTH_MAX_HANDSHAKE_PACKETS: usize = 8;
const POST_AUTH_MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
const MAX_PENDING_SESSION_EVENTS: usize = 8;
const MAX_SESSIONS: usize = 512;
const MAX_INBOUND_UDP_PEERS: usize = MAX_SESSIONS + ACCEPT_BACKLOG;
const MAX_INBOUND_UDP_PEERS_PER_IP: usize = 32;
const MAX_PENDING_UDP_PEERS_PER_IP: usize = 4;
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(super) enum TransportEvent {
    Datagram {
        bytes: Vec<u8>,
        address: SocketAddr,
        session_id: u64,
        accepted: bool,
        received_while_gated: bool,
        certificate: CertificateId,
        active: Arc<AtomicBool>,
        invalidated: Arc<AtomicBool>,
        pending_events: Arc<AtomicUsize>,
    },
    Sent {
        transaction_id: u16,
        address: SocketAddr,
    },
    SendFailed {
        transaction_id: u16,
        address: SocketAddr,
    },
    PeerRevoked {
        session_id: u64,
        address: SocketAddr,
    },
    SessionClosed {
        session_id: u64,
    },
}

enum TransportCommand {
    SendRequest {
        recovery: bool,
        bytes: Vec<u8>,
        address: SocketAddr,
        transaction_id: u16,
    },
    SendResponse {
        recovery: bool,
        bytes: Vec<u8>,
        address: SocketAddr,
        session_id: u64,
    },
    EvictRevoked,
    ResetSessions(flume::Sender<()>),
    Shutdown,
}

enum SessionCommand {
    Write {
        recovery: bool,
        bytes: Vec<u8>,
        transaction: Option<(u16, SocketAddr)>,
    },
    Close,
}

enum WorkerEvent {
    Register {
        id: u64,
        address: SocketAddr,
        certificate: CertificateId,
        sender: mpsc::Sender<SessionCommand>,
        active: Arc<AtomicBool>,
        invalidated: Arc<AtomicBool>,
        generation: u64,
        outbound: bool,
        ready: oneshot::Sender<bool>,
    },
    Closed {
        id: u64,
        address: SocketAddr,
        outbound: bool,
    },
}

struct Session {
    sender: mpsc::Sender<SessionCommand>,
    active: Arc<AtomicBool>,
    invalidated: Arc<AtomicBool>,
    certificate: CertificateId,
    address: SocketAddr,
}

struct InboundPeerSlot {
    id: u64,
    connection: Weak<InboundUdpConn>,
}

#[derive(Default)]
struct InboundPeerRegistry {
    next_id: AtomicU64,
    peers: StdMutex<HashMap<SocketAddr, InboundPeerSlot>>,
}

impl InboundPeerRegistry {
    fn existing(&self, address: &SocketAddr) -> Option<Arc<InboundUdpConn>> {
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let connection = peers
            .get(address)
            .and_then(|slot| slot.connection.upgrade());
        if connection.is_none() {
            peers.remove(address);
        }
        connection
    }

    fn create(
        self: &Arc<Self>,
        socket: Arc<UdpSocket>,
        address: SocketAddr,
    ) -> Option<Arc<InboundUdpConn>> {
        self.create_with(socket, address, || {})
    }

    fn create_with(
        self: &Arc<Self>,
        socket: Arc<UdpSocket>,
        address: SocketAddr,
        after_retain: impl FnOnce(),
    ) -> Option<Arc<InboundUdpConn>> {
        // Declare before the guard so last references are dropped only after
        // unlocking, including on early returns. Drop calls remove().
        let mut connections = Vec::new();
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        peers.retain(|_, slot| {
            if let Some(connection) = slot.connection.upgrade() {
                connections.push(connection);
                true
            } else {
                false
            }
        });
        after_retain();
        if peers.len() >= MAX_INBOUND_UDP_PEERS
            || peers
                .keys()
                .filter(|peer| peer.ip() == address.ip())
                .count()
                >= MAX_INBOUND_UDP_PEERS_PER_IP
            || connections
                .iter()
                .filter(|connection| {
                    connection.remote_addr.ip() == address.ip()
                        && !connection.authenticated.load(Ordering::Acquire)
                })
                .count()
                >= MAX_PENDING_UDP_PEERS_PER_IP
        {
            return None;
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let connection = Arc::new(InboundUdpConn::new(
            id,
            socket,
            address,
            Arc::downgrade(self),
        ));
        peers.insert(
            address,
            InboundPeerSlot {
                id,
                connection: Arc::downgrade(&connection),
            },
        );
        Some(connection)
    }

    fn remove(&self, address: SocketAddr, id: u64) {
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if peers.get(&address).is_some_and(|slot| slot.id == id) {
            peers.remove(&address);
        }
    }

    fn close_all(&self) {
        let connections = {
            let mut peers = self
                .peers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let connections = peers
                .values()
                .filter_map(|slot| slot.connection.upgrade())
                .collect::<Vec<_>>();
            peers.clear();
            connections
        };
        for connection in connections {
            connection.signal_close();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

struct InboundUdpConn {
    id: u64,
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    packet_sender: mpsc::Sender<InboundPacket>,
    packet_receiver: AsyncMutex<mpsc::Receiver<InboundPacket>>,
    close_sender: watch::Sender<bool>,
    closed: AtomicBool,
    authenticated: AtomicBool,
    pre_auth_packets: AtomicUsize,
    pre_auth_bytes: AtomicUsize,
    post_auth_handshake_packets: AtomicUsize,
    post_auth_handshake_bytes: AtomicUsize,
    ingress_gate_state: Arc<IngressGateState>,
    peers: Weak<InboundPeerRegistry>,
}

struct InboundPacket {
    bytes: Vec<u8>,
    received_while_gated: bool,
}

#[derive(Default)]
struct IngressGateState {
    saw_gated_application_data: AtomicBool,
}

impl IngressGateState {
    fn record(&self, count: usize) {
        if count > 0 {
            self.saw_gated_application_data
                .store(true, Ordering::Release);
        }
    }

    fn saw_gated_application_data(&self) -> bool {
        self.saw_gated_application_data.load(Ordering::Acquire)
    }
}

impl InboundUdpConn {
    fn new(
        id: u64,
        socket: Arc<UdpSocket>,
        remote_addr: SocketAddr,
        peers: Weak<InboundPeerRegistry>,
    ) -> Self {
        let (packet_sender, packet_receiver) = mpsc::channel(INBOUND_PACKET_QUEUE_SIZE);
        let (close_sender, _) = watch::channel(false);
        Self {
            id,
            socket,
            remote_addr,
            packet_sender,
            packet_receiver: AsyncMutex::new(packet_receiver),
            close_sender,
            closed: AtomicBool::new(false),
            authenticated: AtomicBool::new(false),
            pre_auth_packets: AtomicUsize::new(0),
            pre_auth_bytes: AtomicUsize::new(0),
            post_auth_handshake_packets: AtomicUsize::new(0),
            post_auth_handshake_bytes: AtomicUsize::new(0),
            ingress_gate_state: Arc::new(IngressGateState::default()),
            peers,
        }
    }

    fn deliver(&self, packet: Vec<u8>, received_while_gated: bool) {
        let authenticated = self.authenticated.load(Ordering::Acquire);
        if self.closed.load(Ordering::Acquire) || !is_allowed_dtls_datagram(&packet, authenticated)
        {
            return;
        }
        if !authenticated
            && (self.pre_auth_packets.fetch_add(1, Ordering::Relaxed) + 1 > PRE_AUTH_MAX_PACKETS
                || self
                    .pre_auth_bytes
                    .fetch_add(packet.len(), Ordering::Relaxed)
                    + packet.len()
                    > PRE_AUTH_MAX_BYTES)
        {
            self.signal_close();
            return;
        }
        if authenticated
            && contains_dtls_handshake_flight(&packet)
            && (self
                .post_auth_handshake_packets
                .fetch_add(1, Ordering::Relaxed)
                + 1
                > POST_AUTH_MAX_HANDSHAKE_PACKETS
                || self
                    .post_auth_handshake_bytes
                    .fetch_add(packet.len(), Ordering::Relaxed)
                    + packet.len()
                    > POST_AUTH_MAX_HANDSHAKE_BYTES)
        {
            // Record headers are unauthenticated. Exhaustion must not let a
            // spoofed handshake flight tear down an established session.
            return;
        }
        let _ = self.packet_sender.try_send(InboundPacket {
            bytes: packet,
            received_while_gated,
        });
    }

    fn mark_authenticated(&self) {
        self.authenticated.store(true, Ordering::Release);
    }

    fn signal_close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.close_sender.send_replace(true);
            if let Some(peers) = self.peers.upgrade() {
                peers.remove(self.remote_addr, self.id);
            }
        }
    }

    fn closed_error() -> UtilError {
        UtilError::ErrUseClosedNetworkConn
    }
}

impl Drop for InboundUdpConn {
    fn drop(&mut self) {
        self.signal_close();
    }
}

#[async_trait]
impl Conn for InboundUdpConn {
    async fn connect(&self, address: SocketAddr) -> std::result::Result<(), UtilError> {
        if address == self.remote_addr {
            Ok(())
        } else {
            Err(UtilError::Other(
                "inbound UDP connection has a fixed remote address".to_string(),
            ))
        }
    }

    async fn recv(&self, buffer: &mut [u8]) -> std::result::Result<usize, UtilError> {
        let mut close_receiver = self.close_sender.subscribe();
        if *close_receiver.borrow() {
            return Err(Self::closed_error());
        }
        let mut packet_receiver = self.packet_receiver.lock().await;
        let packet = tokio::select! {
            packet = packet_receiver.recv() => packet.ok_or_else(Self::closed_error)?,
            _ = close_receiver.changed() => return Err(Self::closed_error()),
        };
        if packet.received_while_gated {
            self.ingress_gate_state
                .record(dtls_application_record_count(&packet.bytes));
        }
        if packet.bytes.len() > buffer.len() {
            return Err(UtilError::ErrBufferShort);
        }
        buffer[..packet.bytes.len()].copy_from_slice(&packet.bytes);
        Ok(packet.bytes.len())
    }

    async fn recv_from(
        &self,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, SocketAddr), UtilError> {
        Ok((self.recv(buffer).await?, self.remote_addr))
    }

    async fn send(&self, buffer: &[u8]) -> std::result::Result<usize, UtilError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Self::closed_error());
        }
        self.socket
            .send_to(buffer, self.remote_addr)
            .await
            .map_err(UtilError::from)
    }

    async fn send_to(
        &self,
        buffer: &[u8],
        target: SocketAddr,
    ) -> std::result::Result<usize, UtilError> {
        if target != self.remote_addr {
            return Err(UtilError::Other(
                "inbound UDP connection cannot send to another peer".to_string(),
            ));
        }
        self.send(buffer).await
    }

    fn local_addr(&self) -> std::result::Result<SocketAddr, UtilError> {
        self.socket.local_addr().map_err(UtilError::from)
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote_addr)
    }

    async fn close(&self) -> std::result::Result<(), UtilError> {
        self.signal_close();
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

struct GateTrackingConn {
    inner: Arc<dyn Conn + Send + Sync>,
    request_gate: Option<Arc<AtomicBool>>,
    ingress_gate_state: Arc<IngressGateState>,
}

impl GateTrackingConn {
    fn new(inner: Arc<dyn Conn + Send + Sync>, request_gate: Option<Arc<AtomicBool>>) -> Self {
        Self {
            inner,
            request_gate,
            ingress_gate_state: Arc::new(IngressGateState::default()),
        }
    }

    fn record_ingress_gate(&self, packet: &[u8]) {
        if self
            .request_gate
            .as_ref()
            .is_some_and(|gate| !gate.load(Ordering::Acquire))
        {
            self.ingress_gate_state
                .record(dtls_application_record_count(packet));
        }
    }
}

#[async_trait]
impl Conn for GateTrackingConn {
    async fn connect(&self, address: SocketAddr) -> std::result::Result<(), UtilError> {
        self.inner.connect(address).await
    }

    async fn recv(&self, buffer: &mut [u8]) -> std::result::Result<usize, UtilError> {
        let size = self.inner.recv(buffer).await?;
        self.record_ingress_gate(&buffer[..size]);
        Ok(size)
    }

    async fn recv_from(
        &self,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, SocketAddr), UtilError> {
        let received = self.inner.recv_from(buffer).await?;
        self.record_ingress_gate(&buffer[..received.0]);
        Ok(received)
    }

    async fn send(&self, buffer: &[u8]) -> std::result::Result<usize, UtilError> {
        self.inner.send(buffer).await
    }

    async fn send_to(
        &self,
        buffer: &[u8],
        target: SocketAddr,
    ) -> std::result::Result<usize, UtilError> {
        self.inner.send_to(buffer, target).await
    }

    fn local_addr(&self) -> std::result::Result<SocketAddr, UtilError> {
        self.inner.local_addr()
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.inner.remote_addr()
    }

    async fn close(&self) -> std::result::Result<(), UtilError> {
        self.inner.close().await
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

struct BoundedUdpListener {
    socket: Arc<UdpSocket>,
    peers: Arc<InboundPeerRegistry>,
    accept_receiver: AsyncMutex<mpsc::Receiver<Arc<InboundUdpConn>>>,
    shutdown_sender: watch::Sender<bool>,
    closed: Arc<AtomicBool>,
    read_task: AsyncMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl BoundedUdpListener {
    async fn bind(
        address: SocketAddr,
        request_gate: Option<Arc<AtomicBool>>,
    ) -> std::result::Result<Self, UtilError> {
        let socket = Arc::new(UdpSocket::bind(address).await.map_err(UtilError::from)?);
        let peers = Arc::new(InboundPeerRegistry::default());
        let (accept_sender, accept_receiver) = mpsc::channel(ACCEPT_BACKLOG);
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);
        let closed = Arc::new(AtomicBool::new(false));
        let read_task = tokio::spawn(run_udp_read_loop(
            Arc::clone(&socket),
            Arc::clone(&peers),
            accept_sender,
            shutdown_receiver,
            Arc::clone(&closed),
            request_gate,
        ));
        Ok(Self {
            socket,
            peers,
            accept_receiver: AsyncMutex::new(accept_receiver),
            shutdown_sender,
            closed,
            read_task: AsyncMutex::new(Some(read_task)),
        })
    }

    async fn accept_raw(
        &self,
    ) -> std::result::Result<(Arc<InboundUdpConn>, SocketAddr), UtilError> {
        let mut shutdown_receiver = self.shutdown_sender.subscribe();
        if *shutdown_receiver.borrow() {
            return Err(UtilError::ErrClosedListener);
        }
        let mut accept_receiver = self.accept_receiver.lock().await;
        tokio::select! {
            connection = accept_receiver.recv() => {
                let connection = connection.ok_or(UtilError::ErrClosedListenerAcceptCh)?;
                let address = connection.remote_addr;
                Ok((connection, address))
            }
            _ = shutdown_receiver.changed() => Err(UtilError::ErrClosedListener),
        }
    }
}

#[async_trait]
impl Listener for BoundedUdpListener {
    async fn accept(
        &self,
    ) -> std::result::Result<(Arc<dyn Conn + Send + Sync>, SocketAddr), UtilError> {
        let (connection, address) = self.accept_raw().await?;
        Ok((connection, address))
    }

    async fn close(&self) -> std::result::Result<(), UtilError> {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.shutdown_sender.send_replace(true);
            self.peers.close_all();
        }
        if let Some(read_task) = self.read_task.lock().await.take() {
            let _ = tokio::time::timeout(SESSION_CLOSE_TIMEOUT, read_task).await;
        }
        self.peers.close_all();
        Ok(())
    }

    async fn addr(&self) -> std::result::Result<SocketAddr, UtilError> {
        self.socket.local_addr().map_err(UtilError::from)
    }
}

async fn run_udp_read_loop(
    socket: Arc<UdpSocket>,
    peers: Arc<InboundPeerRegistry>,
    accept_sender: mpsc::Sender<Arc<InboundUdpConn>>,
    mut shutdown_receiver: watch::Receiver<bool>,
    closed: Arc<AtomicBool>,
    request_gate: Option<Arc<AtomicBool>>,
) {
    let mut buffer = vec![0u8; UDP_RECEIVE_MTU];
    loop {
        let received = tokio::select! {
            result = socket.recv_from(&mut buffer) => result,
            _ = shutdown_receiver.changed() => break,
        };
        let (size, address) = match received {
            Ok(received) => received,
            Err(_) if !closed.load(Ordering::Acquire) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            Err(_) => break,
        };
        if closed.load(Ordering::Acquire) {
            break;
        }

        let connection = match peers.existing(&address) {
            Some(connection) => connection,
            None => {
                if !is_initial_dtls_client_hello(&buffer[..size]) {
                    continue;
                }
                let Some(connection) = peers.create(Arc::clone(&socket), address) else {
                    continue;
                };
                if accept_sender.try_send(Arc::clone(&connection)).is_err() {
                    connection.signal_close();
                    continue;
                }
                connection
            }
        };
        connection.deliver(
            buffer[..size].to_vec(),
            request_gate
                .as_ref()
                .is_some_and(|gate| !gate.load(Ordering::Acquire)),
        );
    }
    peers.close_all();
}

fn is_initial_dtls_client_hello(packet: &[u8]) -> bool {
    const RECORD_HEADER_SIZE: usize = 13;
    const HANDSHAKE_HEADER_SIZE: usize = 12;
    if !is_allowed_dtls_datagram(packet, false)
        || packet.len() < RECORD_HEADER_SIZE + HANDSHAKE_HEADER_SIZE + 2
        || packet[0] != 22
        || packet[3..5] != [0, 0]
        || packet[RECORD_HEADER_SIZE] != 1
        || read_u24(&packet[RECORD_HEADER_SIZE + 6..RECORD_HEADER_SIZE + 9]) != 0
    {
        return false;
    }
    let fragment_length =
        read_u24(&packet[RECORD_HEADER_SIZE + 9..RECORD_HEADER_SIZE + HANDSHAKE_HEADER_SIZE]);
    let body_start = RECORD_HEADER_SIZE + HANDSHAKE_HEADER_SIZE;
    is_structurally_valid_client_hello(&packet[body_start..body_start + fragment_length])
}

fn is_structurally_valid_client_hello(body: &[u8]) -> bool {
    const VERSION_AND_RANDOM_SIZE: usize = 34;
    if body.len() < VERSION_AND_RANDOM_SIZE + 8 || body[..2] != [0xfe, 0xfd] {
        return false;
    }
    let mut offset = VERSION_AND_RANDOM_SIZE;
    let session_id_length = usize::from(body[offset]);
    offset += 1;
    if offset + session_id_length >= body.len() {
        return false;
    }
    offset += session_id_length;
    let cookie_length = usize::from(body[offset]);
    offset += 1;
    if offset + cookie_length + 2 > body.len() {
        return false;
    }
    offset += cookie_length;
    let cipher_suites_length = u16::from_be_bytes([body[offset], body[offset + 1]]) as usize;
    offset += 2;
    if cipher_suites_length == 0
        || cipher_suites_length % 2 != 0
        || offset + cipher_suites_length >= body.len()
    {
        return false;
    }
    offset += cipher_suites_length;
    let compression_methods_length = usize::from(body[offset]);
    compression_methods_length > 0 && offset + 1 + compression_methods_length <= body.len()
}

fn is_allowed_dtls_datagram(packet: &[u8], authenticated: bool) -> bool {
    const RECORD_HEADER_SIZE: usize = 13;
    const HANDSHAKE_HEADER_SIZE: usize = 12;
    let mut offset = 0;
    while offset < packet.len() {
        if packet.len() - offset < RECORD_HEADER_SIZE {
            return false;
        }
        let version = &packet[offset + 1..offset + 3];
        if version != [0xfe, 0xff] && version != [0xfe, 0xfd] {
            return false;
        }
        let epoch = u16::from_be_bytes([packet[offset + 3], packet[offset + 4]]);
        let content_type = packet[offset];
        if epoch > 1 || authenticated && epoch == 0 && !matches!(content_type, 20 | 22) {
            return false;
        }
        let record_length = u16::from_be_bytes([packet[offset + 11], packet[offset + 12]]) as usize;
        let record_end = offset + RECORD_HEADER_SIZE + record_length;
        if record_end > packet.len() {
            return false;
        }
        if epoch == 0 && content_type == 20 {
            if record_length != 1 || packet[offset + RECORD_HEADER_SIZE] != 1 {
                return false;
            }
        }
        if epoch == 0 && content_type == 22 {
            if record_length < HANDSHAKE_HEADER_SIZE {
                return false;
            }
            let handshake = offset + RECORD_HEADER_SIZE;
            let message_length = read_u24(&packet[handshake + 1..handshake + 4]);
            let fragment_offset = read_u24(&packet[handshake + 6..handshake + 9]);
            let fragment_length = read_u24(&packet[handshake + 9..handshake + 12]);
            if fragment_length == 0
                || fragment_offset.saturating_add(fragment_length) > message_length
                || HANDSHAKE_HEADER_SIZE + fragment_length > record_length
            {
                return false;
            }
        }
        offset = record_end;
    }
    !packet.is_empty()
}

fn contains_dtls_handshake_flight(packet: &[u8]) -> bool {
    const RECORD_HEADER_SIZE: usize = 13;
    let mut offset = 0;
    while offset < packet.len() {
        if matches!(packet[offset], 20 | 22) {
            return true;
        }
        let record_length = u16::from_be_bytes([packet[offset + 11], packet[offset + 12]]) as usize;
        offset += RECORD_HEADER_SIZE + record_length;
    }
    false
}

fn dtls_application_record_count(packet: &[u8]) -> usize {
    const RECORD_HEADER_SIZE: usize = 13;
    let mut count = 0;
    let mut offset = 0;
    while packet.len().saturating_sub(offset) >= RECORD_HEADER_SIZE {
        let record_length = u16::from_be_bytes([packet[offset + 11], packet[offset + 12]]) as usize;
        let Some(record_end) = offset
            .checked_add(RECORD_HEADER_SIZE)
            .and_then(|header_end| header_end.checked_add(record_length))
        else {
            break;
        };
        if record_end > packet.len() {
            break;
        }
        if packet[offset] == 23 && packet[offset + 3..offset + 5] != [0, 0] {
            count += 1;
        }
        offset = record_end;
    }
    count
}

fn read_u24(bytes: &[u8]) -> usize {
    (usize::from(bytes[0]) << 16) | (usize::from(bytes[1]) << 8) | usize::from(bytes[2])
}

pub(super) struct DtlsTransport {
    command_sender: Option<flume::Sender<TransportCommand>>,
    event_receiver: flume::Receiver<TransportEvent>,
    control_event_receiver: flume::Receiver<TransportEvent>,
    local_addr: SocketAddr,
    handshake_timeout: Duration,
    worker: Option<JoinHandle<()>>,
}

impl Debug for DtlsTransport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DtlsTransport")
            .field("local_addr", &self.local_addr)
            .field("handshake_timeout", &self.handshake_timeout)
            .finish_non_exhaustive()
    }
}

impl DtlsTransport {
    pub(super) fn new(
        settings: &DhtSettings,
        request_gate: Option<Arc<AtomicBool>>,
    ) -> Result<Self> {
        let config = settings.dtls.clone().ok_or(Error::MissingDtlsConfig)?;
        let bind_port = settings.port.unwrap_or(super::socket::DEFAULT_PORT);
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), bind_port);
        let fallback_to_ephemeral = settings.port.is_none();
        let handshake_timeout = config.handshake_timeout();
        let (command_sender, command_receiver) = flume::bounded(COMMAND_QUEUE_SIZE);
        let (event_sender, event_receiver) = flume::bounded(EVENT_QUEUE_SIZE);
        let (control_event_sender, control_event_receiver) = flume::bounded(EVENT_QUEUE_SIZE);
        let (startup_sender, startup_receiver) = flume::bounded(1);

        let worker = std::thread::Builder::new()
            .name("mainline-dtls".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = startup_sender.send(Err(Error::Dtls(error.to_string())));
                        return;
                    }
                };

                runtime.block_on(run_worker(
                    bind_addr,
                    fallback_to_ephemeral,
                    config,
                    command_receiver,
                    event_sender,
                    control_event_sender,
                    startup_sender,
                    request_gate,
                ));
            })
            .map_err(Error::IO)?;

        let local_addr = startup_receiver.recv()??;
        Ok(Self {
            command_sender: Some(command_sender),
            event_receiver,
            control_event_receiver,
            local_addr,
            handshake_timeout,
            worker: Some(worker),
        })
    }

    pub(super) fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub(super) fn handshake_timeout(&self) -> Duration {
        self.handshake_timeout
    }

    pub(super) fn send_request(
        &self,
        address: SocketAddr,
        transaction_id: u16,
        bytes: Vec<u8>,
        recovery: bool,
    ) -> Result<()> {
        self.command_sender
            .as_ref()
            .ok_or_else(|| Error::Dtls("DTLS transport is shut down".to_string()))?
            .send(TransportCommand::SendRequest {
                recovery,
                bytes,
                address,
                transaction_id,
            })
            .map_err(|error| Error::Dtls(error.to_string()))
    }

    pub(super) fn send_response(
        &self,
        address: SocketAddr,
        session_id: u64,
        bytes: Vec<u8>,
        recovery: bool,
    ) -> Result<()> {
        self.command_sender
            .as_ref()
            .ok_or_else(|| Error::Dtls("DTLS transport is shut down".to_string()))?
            .send(TransportCommand::SendResponse {
                recovery,
                bytes,
                address,
                session_id,
            })
            .map_err(|error| Error::Dtls(error.to_string()))
    }

    pub(super) fn recv_timeout(&self, timeout: Duration) -> Option<TransportEvent> {
        self.control_event_receiver
            .try_recv()
            .ok()
            .or_else(|| self.event_receiver.recv_timeout(timeout).ok())
    }

    #[cfg(test)]
    pub(super) fn pending_datagrams(&self) -> usize {
        self.event_receiver.len()
    }

    pub(super) fn evict_revoked(&self) {
        if let Some(sender) = &self.command_sender {
            let _ = sender.send(TransportCommand::EvictRevoked);
        }
    }

    pub(super) fn reset_sessions(&self) {
        let Some(command_sender) = &self.command_sender else {
            return;
        };
        let (completion_sender, completion_receiver) = flume::bounded(1);
        if command_sender
            .send(TransportCommand::ResetSessions(completion_sender))
            .is_ok()
        {
            let _ = completion_receiver.recv();
        }
    }
}

impl Drop for DtlsTransport {
    fn drop(&mut self) {
        if let Some(sender) = self.command_sender.take() {
            let _ = sender.try_send(TransportCommand::Shutdown);
            drop(sender);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn run_worker(
    bind_addr: SocketAddr,
    fallback_to_ephemeral: bool,
    config: DtlsConfig,
    command_receiver: flume::Receiver<TransportCommand>,
    event_sender: flume::Sender<TransportEvent>,
    control_event_sender: flume::Sender<TransportEvent>,
    startup_sender: flume::Sender<Result<SocketAddr>>,
    request_gate: Option<Arc<AtomicBool>>,
) {
    let listener = match bind_udp_listener(bind_addr, request_gate.clone()).await {
        Ok(listener) => listener,
        Err(_) if fallback_to_ephemeral => {
            let fallback_addr = SocketAddr::new(bind_addr.ip(), 0);
            match bind_udp_listener(fallback_addr, request_gate.clone()).await {
                Ok(listener) => listener,
                Err(error) => {
                    let _ = startup_sender.send(Err(error));
                    return;
                }
            }
        }
        Err(error) => {
            let _ = startup_sender.send(Err(error));
            return;
        }
    };
    let local_addr = match listener.addr().await {
        Ok(address) => address,
        Err(error) => {
            let _ = startup_sender.send(Err(Error::Dtls(error.to_string())));
            return;
        }
    };
    if startup_sender.send(Ok(local_addr)).is_err() {
        let _ = listener.close().await;
        return;
    }

    let (worker_event_sender, mut worker_event_receiver) = mpsc::channel(EVENT_QUEUE_SIZE);
    let next_session_id = Arc::new(AtomicU64::new(1));
    let session_generation = Arc::new(AtomicU64::new(0));
    for _ in 0..ACCEPT_WORKERS {
        let listener = Arc::clone(&listener);
        let config = config.clone();
        let event_sender = event_sender.clone();
        let control_event_sender = control_event_sender.clone();
        let worker_event_sender = worker_event_sender.clone();
        let next_session_id = Arc::clone(&next_session_id);
        let session_generation = Arc::clone(&session_generation);
        let request_gate = request_gate.clone();
        tokio::spawn(async move {
            loop {
                let (raw_connection, address) = match listener.accept_raw().await {
                    Ok(connection) => connection,
                    Err(UtilError::ErrClosedListener)
                    | Err(UtilError::ErrClosedListenerAcceptCh) => break,
                    Err(error) => {
                        tracing::warn!(stage = "accept", direction = "inbound", %error, "DTLS listener accept failed before peer address was available");
                        continue;
                    }
                };
                let generation = session_generation.load(Ordering::Acquire);
                let dtls_connection: Arc<dyn Conn + Send + Sync> = raw_connection.clone();
                let connection = match tokio::time::timeout(
                    config.handshake_timeout(),
                    DTLSConn::new(dtls_connection, config.connection_config(), false, None),
                )
                .await
                {
                    Ok(Ok(connection)) => Arc::new(connection) as Arc<dyn Conn + Send + Sync>,
                    Ok(Err(error)) => {
                        if listener.closed.load(Ordering::Acquire) {
                            tracing::debug!(peer = %address, direction = "inbound", stage = "handshake", %error, "DTLS handshake stopped during shutdown");
                        } else {
                            tracing::warn!(
                                peer = %address, direction = "inbound", stage = "handshake", %error,
                                timeout_ms = config.handshake_timeout().as_millis() as u64,
                                "DTLS handshake failed (including peer certificate verification)"
                            );
                        }
                        raw_connection.signal_close();
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(
                            peer = %address, direction = "inbound", stage = "handshake", %error,
                            timeout_ms = config.handshake_timeout().as_millis() as u64,
                            "DTLS handshake timed out"
                        );
                        raw_connection.signal_close();
                        continue;
                    }
                };
                let certificate = match peer_certificate_id(&config, &connection).await {
                    Ok(certificate) => certificate,
                    Err(error) => {
                        tracing::warn!(peer = %address, direction = "inbound", stage = "certificate_validation", %error, "DTLS peer certificate rejected");
                        close_connection(&connection).await;
                        continue;
                    }
                };
                raw_connection.mark_authenticated();
                let id = next_session_id.fetch_add(1, Ordering::Relaxed);
                let active = Arc::new(AtomicBool::new(true));
                let invalidated = Arc::new(AtomicBool::new(false));
                let (session_sender, session_receiver) = mpsc::channel(SESSION_QUEUE_SIZE);
                let (ready_sender, ready_receiver) = oneshot::channel();
                if worker_event_sender
                    .send(WorkerEvent::Register {
                        id,
                        address,
                        certificate: certificate.clone(),
                        sender: session_sender,
                        active: Arc::clone(&active),
                        invalidated: Arc::clone(&invalidated),
                        generation,
                        outbound: false,
                        ready: ready_sender,
                    })
                    .await
                    .is_err()
                {
                    tracing::debug!(peer = %address, direction = "inbound", stage = "registration", "DTLS worker closed during registration (shutdown)");
                    close_connection(&connection).await;
                    break;
                }
                let ready = ready_receiver.await;
                if ready != Ok(true) {
                    if let Err(error) = ready {
                        tracing::debug!(
                            peer = %address, direction = "inbound", stage = "registration", %error,
                            "DTLS registration acknowledgement dropped (worker shutdown)"
                        );
                    }
                    close_connection(&connection).await;
                    continue;
                }
                tokio::spawn(run_session(
                    id,
                    address,
                    true,
                    connection,
                    certificate,
                    config.revocations().clone(),
                    session_receiver,
                    event_sender.clone(),
                    control_event_sender.clone(),
                    worker_event_sender.clone(),
                    request_gate.clone(),
                    Some(Arc::clone(&raw_connection.ingress_gate_state)),
                    active,
                    invalidated,
                ));
            }
        });
    }

    let mut sessions: HashMap<u64, Session> = HashMap::new();
    let mut outbound_sessions: HashMap<SocketAddr, (u64, mpsc::Sender<SessionCommand>)> =
        HashMap::new();

    loop {
        tokio::select! {
            command = command_receiver.recv_async() => {
                match command {
                    Ok(TransportCommand::SendRequest { bytes, address, transaction_id, recovery }) => {
                        if config.revocations().security_failed() && !recovery {
                            let _ = control_event_sender.try_send(TransportEvent::SendFailed { transaction_id, address });
                            continue;
                        }
                        let Some(sender) = outbound_sender(
                            address,
                            &config,
                            &event_sender,
                            &control_event_sender,
                            &worker_event_sender,
                            &next_session_id,
                            &session_generation,
                            &mut outbound_sessions,
                            &request_gate,
                        ) else {
                            let _ = control_event_sender.try_send(TransportEvent::SendFailed {
                                transaction_id,
                                address,
                            });
                            continue;
                        };
                        if sender.try_send(SessionCommand::Write {
                            recovery,
                            bytes,
                            transaction: Some((transaction_id, address)),
                        }).is_err() {
                            let _ = control_event_sender.try_send(TransportEvent::SendFailed {
                                transaction_id,
                                address,
                            });
                        }
                    }
                    Ok(TransportCommand::SendResponse { bytes, address, session_id, recovery }) => {
                        if let Some(session) = sessions
                            .get(&session_id)
                            .filter(|session| session.address.ip() == address.ip())
                        {
                            let _ = session.sender.try_send(SessionCommand::Write {
                                recovery,
                                bytes,
                                transaction: None,
                            });
                        }
                    }
                    Ok(TransportCommand::EvictRevoked) => {
                        for (session_id, session) in &sessions {
                            if config.revocations().is_revoked(&session.certificate) {
                                let _ = control_event_sender.try_send(TransportEvent::PeerRevoked {
                                    session_id: *session_id,
                                    address: session.address,
                                });
                                let _ = session.sender.try_send(SessionCommand::Close);
                            }
                        }
                    }
                    Ok(TransportCommand::ResetSessions(completion)) => {
                        tracing::debug!(
                            stage = "reset", sessions = sessions.len(),
                            outbound_sessions = outbound_sessions.len(),
                            "Resetting DTLS sessions for serving-gate transition"
                        );
                        session_generation.fetch_add(1, Ordering::AcqRel);
                        for session in sessions.values() {
                            session.active.store(false, Ordering::Release);
                            session.invalidated.store(true, Ordering::Release);
                            let _ = session.sender.try_send(SessionCommand::Close);
                        }
                        outbound_sessions.clear();
                        let _ = completion.send(());
                    }
                    Ok(TransportCommand::Shutdown) | Err(_) => break,
                }
            }
            event = worker_event_receiver.recv() => {
                match event {
                    Some(WorkerEvent::Register {
                        id,
                        address,
                        certificate,
                        sender,
                        active,
                        invalidated,
                        generation,
                        outbound,
                        ready,
                    }) => {
                        let revoked = config.revocations().is_revoked(&certificate);
                        let current_generation = session_generation.load(Ordering::Acquire);
                        let accepted = !revoked
                            && generation == current_generation
                            && sessions.len() < MAX_SESSIONS;
                        let direction = if outbound { "outbound" } else { "inbound" };
                        if revoked {
                            tracing::warn!(
                                peer = %address, direction, stage = "registration",
                                error = "peer certificate revoked",
                                "DTLS session registration rejected"
                            );
                        } else if generation != current_generation {
                            tracing::debug!(
                                peer = %address, direction, stage = "registration",
                                generation, current_generation,
                                "Retiring DTLS session from before serving-gate reset"
                            );
                        } else if !accepted {
                            tracing::warn!(
                                peer = %address, direction, stage = "registration",
                                error = "session limit reached",
                                sessions = sessions.len(), limit = MAX_SESSIONS,
                                "DTLS session registration rejected"
                            );
                        }
                        if accepted {
                            sessions.insert(id, Session {
                                sender: sender.clone(),
                                active,
                                invalidated,
                                certificate,
                                address,
                            });
                            if outbound {
                                outbound_sessions.insert(address, (id, sender));
                            }
                        } else if outbound
                            && outbound_sessions.get(&address).map(|(current, _)| *current) == Some(id)
                        {
                            outbound_sessions.remove(&address);
                        }
                        if revoked {
                            let _ = control_event_sender.try_send(TransportEvent::PeerRevoked {
                                session_id: id,
                                address,
                            });
                        }
                        if ready.send(accepted).is_err() {
                            tracing::debug!(
                                peer = %address, direction, stage = "registration",
                                "DTLS registration requester dropped acknowledgement receiver"
                            );
                        }
                    }
                    Some(WorkerEvent::Closed { id, address, outbound }) => {
                        sessions.remove(&id);
                        if outbound
                            && outbound_sessions.get(&address).map(|(current, _)| *current) == Some(id)
                        {
                            outbound_sessions.remove(&address);
                        }
                    }
                    None => break,
                }
            }
        }
    }

    tracing::debug!(
        stage = "shutdown",
        sessions = sessions.len(),
        "Stopping DTLS transport"
    );
    for session in sessions.values() {
        let _ = session.sender.try_send(SessionCommand::Close);
    }
    let _ = listener.close().await;
}

async fn bind_udp_listener(
    address: SocketAddr,
    request_gate: Option<Arc<AtomicBool>>,
) -> Result<Arc<BoundedUdpListener>> {
    Ok(Arc::new(
        BoundedUdpListener::bind(address, request_gate)
            .await
            .map_err(|error| Error::Dtls(error.to_string()))?,
    ))
}

fn outbound_sender(
    address: SocketAddr,
    config: &DtlsConfig,
    event_sender: &flume::Sender<TransportEvent>,
    control_event_sender: &flume::Sender<TransportEvent>,
    worker_event_sender: &mpsc::Sender<WorkerEvent>,
    next_session_id: &AtomicU64,
    session_generation: &AtomicU64,
    outbound_sessions: &mut HashMap<SocketAddr, (u64, mpsc::Sender<SessionCommand>)>,
    request_gate: &Option<Arc<AtomicBool>>,
) -> Option<mpsc::Sender<SessionCommand>> {
    if let Some((_, sender)) = outbound_sessions.get(&address) {
        return Some(sender.clone());
    }
    if outbound_sessions.len() >= MAX_SESSIONS {
        return None;
    }

    let id = next_session_id.fetch_add(1, Ordering::Relaxed);
    let generation = session_generation.load(Ordering::Acquire);
    let active = Arc::new(AtomicBool::new(true));
    let invalidated = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel(SESSION_QUEUE_SIZE);
    outbound_sessions.insert(address, (id, sender.clone()));
    tokio::spawn(run_outbound_session(
        id,
        address,
        config.clone(),
        receiver,
        sender.clone(),
        event_sender.clone(),
        control_event_sender.clone(),
        worker_event_sender.clone(),
        request_gate.clone(),
        generation,
        active,
        invalidated,
    ));
    Some(sender)
}

async fn run_outbound_session(
    id: u64,
    address: SocketAddr,
    config: DtlsConfig,
    mut receiver: mpsc::Receiver<SessionCommand>,
    sender: mpsc::Sender<SessionCommand>,
    event_sender: flume::Sender<TransportEvent>,
    control_event_sender: flume::Sender<TransportEvent>,
    worker_event_sender: mpsc::Sender<WorkerEvent>,
    request_gate: Option<Arc<AtomicBool>>,
    generation: u64,
    active: Arc<AtomicBool>,
    invalidated: Arc<AtomicBool>,
) {
    let (connection, ingress_gate_state) =
        match connect(address, &config, request_gate.clone()).await {
            Ok(connection) => connection,
            Err(_) => {
                receiver.close();
                fail_pending_requests(&mut receiver, &control_event_sender).await;
                let _ = worker_event_sender
                    .send(WorkerEvent::Closed {
                        id,
                        address,
                        outbound: true,
                    })
                    .await;
                return;
            }
        };
    let certificate = match peer_certificate_id(&config, &connection).await {
        Ok(certificate) => certificate,
        Err(error) => {
            tracing::warn!(peer = %address, direction = "outbound", stage = "certificate_validation", %error, "DTLS peer certificate rejected");
            receiver.close();
            close_connection(&connection).await;
            fail_pending_requests(&mut receiver, &control_event_sender).await;
            let _ = worker_event_sender
                .send(WorkerEvent::Closed {
                    id,
                    address,
                    outbound: true,
                })
                .await;
            return;
        }
    };
    let (ready_sender, ready_receiver) = oneshot::channel();
    if worker_event_sender
        .send(WorkerEvent::Register {
            id,
            address,
            certificate: certificate.clone(),
            sender,
            active: Arc::clone(&active),
            invalidated: Arc::clone(&invalidated),
            generation,
            outbound: true,
            ready: ready_sender,
        })
        .await
        .is_err()
    {
        tracing::debug!(peer = %address, direction = "outbound", stage = "registration", "DTLS worker closed during registration (shutdown)");
        receiver.close();
        close_connection(&connection).await;
        fail_pending_requests(&mut receiver, &control_event_sender).await;
        return;
    }
    let ready = ready_receiver.await;
    if ready != Ok(true) {
        if let Err(error) = ready {
            tracing::debug!(
                peer = %address, direction = "outbound", stage = "registration", %error,
                "DTLS registration acknowledgement dropped (worker shutdown)"
            );
        }
        receiver.close();
        close_connection(&connection).await;
        fail_pending_requests(&mut receiver, &control_event_sender).await;
        let _ = worker_event_sender
            .send(WorkerEvent::Closed {
                id,
                address,
                outbound: true,
            })
            .await;
        return;
    }

    run_session(
        id,
        address,
        false,
        connection,
        certificate,
        config.revocations().clone(),
        receiver,
        event_sender,
        control_event_sender,
        worker_event_sender,
        request_gate,
        Some(ingress_gate_state),
        active,
        invalidated,
    )
    .await;
}

async fn connect(
    address: SocketAddr,
    config: &DtlsConfig,
    request_gate: Option<Arc<AtomicBool>>,
) -> Result<(Arc<dyn Conn + Send + Sync>, Arc<IngressGateState>)> {
    let bind_address = match address {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => {
            tracing::warn!(peer = %address, direction = "outbound", stage = "udp_bind", error = "IPv6 DTLS is not supported", "DTLS connection failed");
            return Err(Error::Dtls("IPv6 DTLS is not supported".to_string()));
        }
    };
    let socket = Arc::new(UdpSocket::bind(bind_address).await.map_err(|error| {
        tracing::warn!(peer = %address, direction = "outbound", stage = "udp_bind", %error, "DTLS connection failed");
        Error::IO(error)
    })?);
    socket.connect(address).await.map_err(|error| {
        tracing::warn!(peer = %address, direction = "outbound", stage = "udp_connect", %error, "DTLS connection failed");
        Error::IO(error)
    })?;
    let socket: Arc<dyn Conn + Send + Sync> = socket;
    let tracked_socket = Arc::new(GateTrackingConn::new(socket, request_gate));
    let dtls_socket: Arc<dyn Conn + Send + Sync> = tracked_socket.clone();
    let connection = tokio::time::timeout(
        config.handshake_timeout(),
        DTLSConn::new(dtls_socket, config.connection_config(), true, None),
    )
    .await
    .map_err(|error| {
        tracing::warn!(
            peer = %address, direction = "outbound", stage = "handshake", %error,
            timeout_ms = config.handshake_timeout().as_millis() as u64,
            "DTLS handshake timed out"
        );
        Error::Dtls("DTLS handshake timed out".to_string())
    })?
    .map_err(|error| {
        tracing::warn!(
            peer = %address, direction = "outbound", stage = "handshake", %error,
            timeout_ms = config.handshake_timeout().as_millis() as u64,
            "DTLS handshake failed (including peer certificate verification)"
        );
        Error::Dtls(error.to_string())
    })?;
    Ok((
        Arc::new(connection),
        Arc::clone(&tracked_socket.ingress_gate_state),
    ))
}

async fn peer_certificate_id(
    config: &DtlsConfig,
    connection: &Arc<dyn Conn + Send + Sync>,
) -> Result<CertificateId> {
    let connection = connection
        .as_any()
        .downcast_ref::<DTLSConn>()
        .ok_or_else(|| Error::Dtls("connection is not DTLS".to_string()))?;
    let state = connection.connection_state().await;
    let certificate = state
        .peer_certificates
        .first()
        .ok_or_else(|| Error::InvalidDtlsPeerCertificate("certificate is missing".to_string()))?;
    config.certificate_id(certificate)
}

fn session_requires_restart(
    ingress_gate_state: &Option<Arc<IngressGateState>>,
    request_gate: &Option<Arc<AtomicBool>>,
) -> bool {
    ingress_gate_state
        .as_ref()
        .is_some_and(|state| state.saw_gated_application_data())
        && request_gate
            .as_ref()
            .is_some_and(|gate| gate.load(Ordering::Acquire))
}

async fn run_session(
    id: u64,
    address: SocketAddr,
    accepted: bool,
    connection: Arc<dyn Conn + Send + Sync>,
    certificate: CertificateId,
    revocations: RevocationStore,
    mut receiver: mpsc::Receiver<SessionCommand>,
    event_sender: flume::Sender<TransportEvent>,
    control_event_sender: flume::Sender<TransportEvent>,
    worker_event_sender: mpsc::Sender<WorkerEvent>,
    request_gate: Option<Arc<AtomicBool>>,
    ingress_gate_state: Option<Arc<IngressGateState>>,
    active: Arc<AtomicBool>,
    invalidated: Arc<AtomicBool>,
) {
    let mut buffer = vec![0u8; KRPC_DATAGRAM_SIZE];
    let pending_events = Arc::new(AtomicUsize::new(0));
    loop {
        if !active.load(Ordering::Acquire)
            || session_requires_restart(&ingress_gate_state, &request_gate)
        {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(SESSION_IDLE_TIMEOUT) => break,
            command = receiver.recv() => {
                match command {
                    Some(SessionCommand::Write { bytes, transaction, recovery }) => {
                        if revocations.security_failed() && !recovery {
                            if let Some((transaction_id, address)) = transaction {
                                let _ = control_event_sender.try_send(TransportEvent::SendFailed { transaction_id, address });
                            }
                            continue;
                        }
                        if !active.load(Ordering::Acquire)
                            || session_requires_restart(&ingress_gate_state, &request_gate)
                        {
                            if let Some((transaction_id, address)) = transaction {
                                let _ = control_event_sender.try_send(TransportEvent::SendFailed {
                                    transaction_id,
                                    address,
                                });
                            }
                            break;
                        }
                        if revocations.is_revoked(&certificate) {
                            let _ = control_event_sender.try_send(TransportEvent::PeerRevoked {
                                session_id: id,
                                address,
                            });
                            break;
                        }
                        match connection.send(&bytes).await {
                            Ok(_) => {
                                if let Some((transaction_id, address)) = transaction {
                                    let _ = control_event_sender.try_send(TransportEvent::Sent {
                                        transaction_id,
                                        address,
                                    });
                                }
                            }
                            Err(_) => {
                                if let Some((transaction_id, address)) = transaction {
                                    let _ = control_event_sender.try_send(TransportEvent::SendFailed {
                                        transaction_id,
                                        address,
                                    });
                                }
                                break;
                            }
                        }
                    }
                    Some(SessionCommand::Close) | None => break,
                }
            }
            received = connection.recv(&mut buffer) => {
                match received {
                    Ok(size) => {
                        if !active.load(Ordering::Acquire)
                            || session_requires_restart(&ingress_gate_state, &request_gate)
                        {
                            break;
                        }
                        if revocations.is_revoked(&certificate) {
                            let _ = control_event_sender.try_send(TransportEvent::PeerRevoked {
                                session_id: id,
                                address,
                            });
                            break;
                        }
                        let received_while_gated = ingress_gate_state
                            .as_ref()
                            .is_some_and(|state| state.saw_gated_application_data())
                            || request_gate
                                .as_ref()
                                .is_some_and(|gate| !gate.load(Ordering::Acquire));
                        if pending_events.fetch_add(1, Ordering::AcqRel)
                            >= MAX_PENDING_SESSION_EVENTS
                        {
                            pending_events.fetch_sub(1, Ordering::AcqRel);
                            continue;
                        }
                        if event_sender.try_send(TransportEvent::Datagram {
                            bytes: buffer[..size].to_vec(),
                            address,
                            session_id: id,
                            accepted,
                            received_while_gated,
                            certificate: certificate.clone(),
                            active: Arc::clone(&active),
                            invalidated: Arc::clone(&invalidated),
                            pending_events: Arc::clone(&pending_events),
                        }).is_err() {
                            pending_events.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
    if !active.load(Ordering::Acquire)
        || session_requires_restart(&ingress_gate_state, &request_gate)
    {
        tracing::debug!(
            peer = %address, direction = if accepted { "inbound" } else { "outbound" },
            stage = "gate_retirement", session_id = id,
            "Retiring DTLS session after serving-gate transition/reset"
        );
    }
    receiver.close();
    active.store(false, Ordering::Release);
    close_connection(&connection).await;
    let _ = control_event_sender.try_send(TransportEvent::SessionClosed { session_id: id });
    fail_pending_requests(&mut receiver, &control_event_sender).await;
    let _ = worker_event_sender
        .send(WorkerEvent::Closed {
            id,
            address,
            outbound: !accepted,
        })
        .await;
}

async fn close_connection(connection: &Arc<dyn Conn + Send + Sync>) {
    let _ = tokio::time::timeout(SESSION_CLOSE_TIMEOUT, connection.close()).await;
}

async fn fail_pending_requests(
    receiver: &mut mpsc::Receiver<SessionCommand>,
    event_sender: &flume::Sender<TransportEvent>,
) {
    while let Ok(command) = receiver.try_recv() {
        if let SessionCommand::Write {
            transaction: Some((transaction_id, address)),
            ..
        } = command
        {
            let _ = event_sender.try_send(TransportEvent::SendFailed {
                transaction_id,
                address,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_hello_record() -> Vec<u8> {
        let body_length = 42;
        let record_length = 12 + body_length;
        let mut packet = vec![0; 13 + record_length];
        packet[0] = 22;
        packet[1] = 0xfe;
        packet[2] = 0xfd;
        packet[11..13].copy_from_slice(&(record_length as u16).to_be_bytes());
        packet[13] = 1;
        packet[16] = body_length as u8;
        packet[24] = body_length as u8;
        packet[25] = 0xfe;
        packet[26] = 0xfd;
        packet[62] = 2;
        packet[63] = 0xc0;
        packet[64] = 0x2b;
        packet[65] = 1;
        packet
    }

    fn dtls_record(content_type: u8, epoch: u16, payload: &[u8]) -> Vec<u8> {
        let mut record = vec![0; 13 + payload.len()];
        record[0] = content_type;
        record[1] = 0xfe;
        record[2] = 0xfd;
        record[3..5].copy_from_slice(&epoch.to_be_bytes());
        record[11..13].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        record[13..].copy_from_slice(payload);
        record
    }

    fn handshake_payload(message_type: u8) -> Vec<u8> {
        let mut payload = vec![0; 13];
        payload[0] = message_type;
        payload[3] = 1;
        payload[11] = 1;
        payload
    }

    async fn admit_connection(
        listener: &BoundedUdpListener,
        sender: &UdpSocket,
        listener_address: SocketAddr,
    ) -> (Arc<dyn Conn + Send + Sync>, SocketAddr) {
        sender
            .send_to(&client_hello_record(), listener_address)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .unwrap()
            .unwrap()
    }

    #[test]
    fn accepts_dtls_compatibility_record_version() {
        let mut packet = client_hello_record();
        packet[2] = 0xff;

        assert!(is_initial_dtls_client_hello(&packet));
    }

    #[test]
    fn rejects_future_epoch_packets() {
        let mut packet = vec![0; 13];
        packet[0] = 23;
        packet[1] = 0xfe;
        packet[2] = 0xfd;
        packet[4] = 2;

        assert!(!is_allowed_dtls_datagram(&packet, false));
        assert!(!is_allowed_dtls_datagram(&packet, true));

        packet[4] = 1;
        packet[0] = 22;
        assert!(is_allowed_dtls_datagram(&packet, true));
    }

    #[test]
    fn accepts_mixed_epoch_final_flight_retransmission() {
        let mut packet = dtls_record(22, 0, &handshake_payload(16));
        packet.extend(dtls_record(20, 0, &[1]));
        packet.extend(dtls_record(22, 1, &[0; 16]));

        assert!(is_allowed_dtls_datagram(&packet, true));
        assert!(contains_dtls_handshake_flight(&packet));
    }

    #[test]
    fn rejects_epoch_zero_application_data_after_authentication() {
        let packet = dtls_record(23, 0, &[0; 16]);

        assert!(!is_allowed_dtls_datagram(&packet, true));
    }

    #[test]
    fn pre_auth_packet_budget_closes_connection() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let socket = Arc::new(
                    UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                        .await
                        .unwrap(),
                );
                let peers = Arc::new(InboundPeerRegistry::default());
                let connection = peers
                    .create(
                        socket,
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345),
                    )
                    .unwrap();
                let mut packet = vec![0; 13];
                packet[0] = 23;
                packet[1] = 0xfe;
                packet[2] = 0xfd;
                packet[4] = 1;

                for _ in 0..=PRE_AUTH_MAX_PACKETS {
                    connection.deliver(packet.clone(), false);
                }

                assert!(connection.closed.load(Ordering::Acquire));
                assert_eq!(peers.len(), 0);
            });
    }

    #[test]
    fn post_auth_handshake_budget_drops_excess_without_closing_connection() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let socket = Arc::new(
                    UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                        .await
                        .unwrap(),
                );
                let peers = Arc::new(InboundPeerRegistry::default());
                let connection = peers
                    .create(
                        socket,
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345),
                    )
                    .unwrap();
                connection.mark_authenticated();
                let mut packet = vec![0; 13];
                packet[0] = 22;
                packet[1] = 0xfe;
                packet[2] = 0xfd;
                packet[4] = 1;

                for _ in 0..=POST_AUTH_MAX_HANDSHAKE_PACKETS {
                    connection.deliver(packet.clone(), false);
                }

                assert!(!connection.closed.load(Ordering::Acquire));
                assert_eq!(peers.len(), 1);
                let mut buffer = [0; 32];
                for _ in 0..POST_AUTH_MAX_HANDSHAKE_PACKETS {
                    assert_eq!(connection.recv(&mut buffer).await.unwrap(), packet.len());
                }
                packet[0] = 23;
                connection.deliver(packet.clone(), false);
                assert_eq!(connection.recv(&mut buffer).await.unwrap(), packet.len());
            });
    }

    #[test]
    fn peer_creation_drops_last_upgraded_references_after_unlock() {
        for count in [1, MAX_PENDING_UDP_PEERS_PER_IP] {
            let (done, completion) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
                        let peers = Arc::new(InboundPeerRegistry::default());
                        let connections: Vec<_> = (0..count)
                            .map(|index| {
                                peers
                                    .create(
                                        socket.clone(),
                                        SocketAddr::from(([127, 0, 0, 1], 12000 + index as u16)),
                                    )
                                    .unwrap()
                            })
                            .collect();
                        let weak: Vec<_> = connections.iter().map(Arc::downgrade).collect();
                        let created = peers.create_with(
                            socket,
                            SocketAddr::from(([127, 0, 0, 1], 13000)),
                            || {
                                // Simulate the other owners disappearing while create holds the mutex.
                                assert!(connections
                                    .iter()
                                    .all(|connection| Arc::strong_count(connection) == 2));
                                drop(connections);
                            },
                        );
                        assert_eq!(created.is_some(), count < MAX_PENDING_UDP_PEERS_PER_IP);
                        assert!(weak.iter().all(|connection| connection.strong_count() == 0));
                        assert_eq!(peers.len(), usize::from(created.is_some()));
                    });
                done.send(()).unwrap();
            });
            completion
                .recv_timeout(Duration::from_secs(5))
                .expect("peer creation deadlocked while dropping retained connections");
            worker.join().unwrap();
        }
    }

    #[test]
    fn preserves_gate_state_from_udp_ingress() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let socket = Arc::new(
                    UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                        .await
                        .unwrap(),
                );
                let peers = Arc::new(InboundPeerRegistry::default());
                let connection = peers
                    .create(
                        socket,
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345),
                    )
                    .unwrap();
                connection.mark_authenticated();
                let mut packet = dtls_record(23, 1, &[0; 16]);
                packet.extend(dtls_record(23, 1, &[0; 16]));
                connection.deliver(packet, true);

                let mut buffer = [0; 64];
                connection.recv(&mut buffer).await.unwrap();

                assert!(connection.ingress_gate_state.saw_gated_application_data());
                assert!(connection.ingress_gate_state.saw_gated_application_data());
            });
    }

    #[test]
    fn closed_inbound_connection_releases_source_tuple() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = BoundedUdpListener::bind(
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
                    None,
                )
                .await
                .unwrap();
                let listener_address = listener.addr().await.unwrap();
                let sender = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                    .await
                    .unwrap();

                sender.send_to(&[0], listener_address).await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
                assert_eq!(listener.peers.len(), 0);

                let (connection, first_address) =
                    admit_connection(&listener, &sender, listener_address).await;
                assert_eq!(first_address, sender.local_addr().unwrap());
                assert_eq!(listener.peers.len(), 1);
                drop(connection);

                for _ in 0..10 {
                    if listener.peers.len() == 0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert_eq!(listener.peers.len(), 0);

                let (connection, second_address) =
                    admit_connection(&listener, &sender, listener_address).await;
                assert_eq!(second_address, first_address);
                connection.close().await.unwrap();
                listener.close().await.unwrap();
            });
    }
}
