//! Manage iterative queries and their corresponding request/response.

use std::collections::HashSet;
use std::net::SocketAddr;

use flume::Sender;
use tracing::{debug, error, info, trace, warn};

use super::socket::KrpcSocket;
use crate::Error;
use crate::{
    common::{
        ErrorSpecific, Id, Node, PutRequest, PutRequestSpecific, RequestSpecific,
        RequestTypeSpecific, RoutingTable,
    },
    rpc::{PutResult, Response, ResponseSender},
};

/// A query is an iterative process of concurrently sending a request to the closest known nodes to
/// the target, updating the routing table with closer nodes discovered in the responses, and
/// repeating this process until no closer nodes (that aren't already queried) are found.
#[derive(Debug)]
pub struct Query {
    revocations: crate::security::RevocationStore,
    recovery: bool,
    pub target: Id,
    pub request: RequestSpecific,
    candidates: RoutingTable,
    responders: RoutingTable,
    inflight_requests: Vec<u16>,
    visited: HashSet<SocketAddr>,
    senders: Vec<ResponseSender>,
    responses: Vec<Response>,
}

impl Query {
    pub fn new(
        target: Id,
        request: RequestSpecific,
        revocations: crate::security::RevocationStore,
        recovery: bool,
    ) -> Self {
        trace!(?target, ?request, "New Query");

        Self {
            revocations,
            recovery,
            target,
            request,

            candidates: RoutingTable::new().with_id(target),
            responders: RoutingTable::new().with_id(target),

            inflight_requests: Vec::with_capacity(200),
            visited: HashSet::with_capacity(200),

            senders: Vec::with_capacity(1),
            responses: Vec::with_capacity(30),
        }
    }

    // === Getters ===

    /// Return the closest responding nodes after the query is done.
    pub fn closest(&self) -> Vec<Node> {
        if self.allowed() {
            self.responders.closest(&self.target)
        } else {
            Vec::new()
        }
    }

    pub fn allowed(&self) -> bool {
        self.recovery || !self.revocations.security_failed()
    }

    // === Public Methods ===

    /// Add a sender to the query and send all replies we found so far to it.
    pub fn add_sender(&mut self, sender: ResponseSender) {
        if !self.allowed() {
            return;
        }
        for response in &self.responses {
            self.send_value(&sender, response.clone())
        }

        self.senders.push(sender);
    }

    /// Force start query traversal by visiting closest nodes.
    pub fn start(&mut self, socket: &mut KrpcSocket) {
        self.visit_closest(socket);
    }

    /// Add a candidate node to query on next tick if it is among the closest nodes.
    pub fn add_candidate(&mut self, node: Node) {
        // ready for a ipv6 routing table?
        self.candidates.add(node);
    }

    /// Visit explicitly given addresses, and add them to the visited set.
    /// only used from the Rpc when calling bootstrapping nodes.
    pub fn visit(&mut self, socket: &mut KrpcSocket, address: SocketAddr) {
        if address.is_ipv6() || !self.allowed() {
            return;
        }

        let tid = socket.request(address, self.request.clone());
        self.inflight_requests.push(tid);
        self.visited.insert(address);
    }

    /// Return true if a response (by transaction_id) is expected by this query.
    pub fn inflight(&self, tid: u16) -> bool {
        self.inflight_requests.contains(&tid)
    }

    /// Add a node that responded with a token as a probable storage node.
    pub fn add_responding_node(&mut self, node: Node) {
        self.responders.add(node.clone());
    }

    /// Add received response
    pub fn response(&mut self, from: SocketAddr, response: Response) {
        if !self.allowed() {
            return;
        }
        let target = self.target;

        debug!(?target, ?response, ?from, "Query got response");

        for sender in &self.senders {
            self.send_value(sender, response.to_owned())
        }

        self.responses.push(response.to_owned());
    }

    /// Query closest nodes for this query's target and message.
    ///
    /// Returns true if it is done.
    pub fn tick(&mut self, socket: &mut KrpcSocket) -> bool {
        if !self.allowed() {
            self.senders.clear();
            self.responses.clear();
            return true;
        }
        // Visit closest nodes
        self.visit_closest(socket);

        // If no more inflight_requests are inflight in the socket (not timed out),
        // then the query is done.
        !self
            .inflight_requests
            .iter()
            .any(|&tid| socket.inflight(&tid))
    }

    // === Private Methods ===

    fn send_value(&self, sender: &ResponseSender, response: Response) {
        self.revocations
            .with_trust(self.recovery, || match (sender, response) {
                (ResponseSender::Peers(s), Response::Peers(r)) => {
                    let _ = s.try_send(r);
                }
                (ResponseSender::Mutable(s), Response::Mutable(r)) => {
                    let _ = s.try_send(r);
                }
                (ResponseSender::Immutable(s), Response::Immutable(r)) => {
                    let _ = s.try_send(r);
                }
                _ => {}
            });
    }

    /// Visit the closest candidates and remove them as candidates
    fn visit_closest(&mut self, socket: &mut KrpcSocket) {
        for node in self.candidates.closest(&self.target) {
            if !self.visited.contains(&node.address) {
                self.visit(socket, node.address);
            }
        }
    }
}

#[derive(Debug)]
pub struct PutQuery {
    revocations: crate::security::RevocationStore,
    recovery: bool,
    pub target: Id,
    /// Nodes that confirmed success
    stored_at: u8,
    inflight_requests: Vec<u16>,
    sender: Option<Sender<PutResult>>,
    request: PutRequestSpecific,
    error: Option<ErrorSpecific>,
    started: bool,
}

impl PutQuery {
    pub fn new(
        target: Id,
        request: PutRequestSpecific,
        sender: Option<Sender<PutResult>>,
        revocations: crate::security::RevocationStore,
        recovery: bool,
    ) -> Self {
        Self {
            revocations,
            recovery,
            target,
            stored_at: 0,
            inflight_requests: Vec::new(),
            sender,
            request,
            error: None,
            started: false,
        }
    }

    pub fn start(&mut self, socket: &mut KrpcSocket, nodes: Vec<Node>) {
        if self.started {
            panic!("should not call PutQuery.start() twice");
        };
        self.started = true;

        let target = self.target;
        trace!(?target, "PutQuery start");

        if let Some(sender) = &self.sender {
            if nodes.is_empty() {
                tracing::warn!(
                    stage = "put_lookup",
                    closest_nodes = 0,
                    "NoClosestNodes: no storage candidates found; check bootstrap peers, UDP reachability, DTLS authentication, and serving readiness"
                );
                let _ = sender.send(Err(Error::NoClosestNodes));
                self.sender = None;
            }
        }

        for node in nodes {
            // Set correct values to the request placeholders
            if let Some(token) = node.token.clone() {
                let tid = socket.request(
                    node.address,
                    RequestSpecific {
                        requester_id: Id::random(),
                        request_type: RequestTypeSpecific::Put(PutRequest {
                            token,
                            put_request_type: self.request.clone(),
                        }),
                    },
                );

                self.inflight_requests.push(tid);
            }
        }
    }

    pub fn inflight(&self, tid: u16) -> bool {
        self.inflight_requests.contains(&tid)
    }

    pub fn success(&mut self) {
        debug!(target = ?self.target, "PutQuery got success response");
        self.stored_at += 1
    }

    pub fn error(&mut self, error: ErrorSpecific) {
        if self.recovery && error.code == 302 {
            debug!(target = ?self.target, ?error,
                "Protected PUT rejected by sequence; readback must confirm the current artifact");
        } else if error.code >= 300 && error.code < 400 {
            warn!(target = ?self.target, ?error, "PutQuery got 3xx error");
        } else {
            debug!(target = ?self.target, ?error, "PutQuery got non-3xx error");
        }
        self.error = Some(error);
    }

    /// Check if the query is done, and if so send the query target to the receiver if any.
    pub fn tick(&mut self, socket: &mut KrpcSocket) -> bool {
        if !self.recovery && self.revocations.security_failed() {
            if let Some(sender) = self.sender.take() {
                let _ = sender.try_send(Err(Error::SecurityFailed));
            }
            return true;
        }
        if
        // Already started
        self.started
        // And all queries got responses or timedout
            && !self
                .inflight_requests
                .iter()
                .any(|&tid| socket.inflight(&tid))
        {
            let target = self.target;
            if self.stored_at == 0 {
                if let Some(sender) = self.sender.take() {
                    let error = self
                        .error
                        .clone()
                        .map(Error::QueryError)
                        .unwrap_or(Error::QueryTimedOut);
                    if self.recovery && matches!(&error, Error::QueryError(reply) if reply.code == 302) {
                        debug!(?target, ?error,
                            "Protected PUT not accepted; awaiting application readback verification");
                    } else {
                        error!(?target, ?error, "Put Query: failed");
                    }
                    let _ = sender.send(Err(error));
                }
            } else {
                info!(?target, stored_at = ?self.stored_at, "PutQuery Done");

                if let Some(sender) = self.sender.take() {
                    if self
                        .revocations
                        .with_trust(self.recovery, || sender.try_send(Ok(target)))
                        .is_none()
                    {
                        let _ = sender.try_send(Err(Error::SecurityFailed));
                    }
                }
            }

            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatal_latch_blocks_cached_replay_and_live_results_and_cancels_get() {
        let mut socket = KrpcSocket::new(&crate::dht::DhtSettings {
            port: Some(0),
            dtls: Some(crate::security::test_dtls_configs(1).unwrap().remove(0)),
            ..Default::default()
        })
        .unwrap();
        let target = Id::random();
        let mut query = Query::new(
            target,
            RequestSpecific {
                requester_id: Id::random(),
                request_type: RequestTypeSpecific::GetValue(
                    crate::common::GetValueRequestArguments {
                        target,
                        seq: None,
                        salt: None,
                    },
                ),
            },
            socket.revocations(),
            false,
        );
        let from = SocketAddr::from(([127, 0, 0, 1], 12345));
        query.response(from, Response::Immutable(vec![1].into()));
        let (sender, receiver) = flume::unbounded();
        query.add_sender(ResponseSender::Immutable(sender));
        assert_eq!(receiver.try_recv().unwrap().as_ref(), &[1]);
        socket.revocations().fail_security();
        let (sender, replay) = flume::unbounded();
        query.add_sender(ResponseSender::Immutable(sender));
        assert!(matches!(
            replay.try_recv(),
            Err(flume::TryRecvError::Disconnected)
        ));
        query.response(from, Response::Immutable(vec![2].into()));
        assert!(receiver.try_recv().is_err());
        assert!(query.tick(&mut socket));
        assert!(query.responses.is_empty());
        assert!(receiver.is_disconnected());
    }

    #[test]
    fn fatal_latch_rejects_confirmed_ordinary_put_but_preserves_recovery_completion() {
        let mut socket = KrpcSocket::new(&crate::dht::DhtSettings {
            port: Some(0),
            dtls: Some(crate::security::test_dtls_configs(1).unwrap().remove(0)),
            ..Default::default()
        })
        .unwrap();
        let target = Id::random();
        for recovery in [false, true] {
            let (sender, receiver) = flume::bounded(1);
            let mut query = PutQuery::new(
                target,
                PutRequestSpecific::PutMutable(crate::common::PutMutableRequestArguments {
                    target,
                    v: vec![1],
                    k: vec![0; 32],
                    sig: vec![0; 64],
                    seq: 1,
                    salt: None,
                    cas: None,
                }),
                Some(sender),
                socket.revocations(),
                recovery,
            );
            query.started = true;
            query.success();
            socket.revocations().fail_security();
            assert!(query.tick(&mut socket));
            let result = receiver.recv().unwrap();
            if recovery {
                assert_eq!(result.unwrap(), target);
            } else {
                assert!(matches!(result, Err(Error::SecurityFailed)));
            }
        }
    }

    #[test]
    fn unanswered_put_reports_timeout_instead_of_channel_closure() {
        let mut socket = KrpcSocket::new(&crate::dht::DhtSettings {
            port: Some(0),
            dtls: Some(crate::security::test_dtls_configs(1).unwrap().remove(0)),
            ..Default::default()
        })
        .unwrap();
        let (sender, receiver) = flume::bounded(1);
        let mut query = PutQuery::new(
            Id::random(),
            PutRequestSpecific::PutImmutable(crate::common::PutImmutableRequestArguments {
                target: Id::random(),
                v: vec![1],
            }),
            Some(sender),
            socket.revocations(),
            false,
        );
        query.started = true;
        assert!(query.tick(&mut socket));
        assert!(matches!(
            receiver.recv().unwrap(),
            Err(Error::QueryTimedOut)
        ));
    }
}
