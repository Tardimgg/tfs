use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use mainline::server::MutableItemValidator;
use mainline::{Id, MutableItem, RevocationSnapshot, RevocationStore};
use openssl::hash::{hash, MessageDigest};
use openssl::pkey::{Id as KeyId, PKey, Public};
use openssl::rsa::Padding;
use openssl::sign::{RsaPssSaltlen, Verifier};
use openssl::x509::X509;
use prost::Message;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::services::dht_map::codec::{decode_enveloped_message, encode_enveloped_message_with_level, MAX_DHT_VALUE_SIZE};
use crate::services::dht_map::dht_map::{generate_key, DHTMap};
use crate::services::dht_map::error::{DhtCodecError, DhtGetError, DhtPutError};

pub(crate) const REVOCATION_DHT_KEY: &str = "__tfs/security/dtls/revocations";
pub(crate) const REVOCATION_SIGNATURE_HEADER: &str = "x-tfs-revocation-signature";
const SIGNATURE_DOMAIN: &[u8] = b"tfs-dtls-revocations\0";
const REVOCATION_ZSTD_LEVEL: i32 = 19;

#[derive(Clone, PartialEq, Message)]
struct SignedRevocationRecord {
    #[prost(bytes = "vec", tag = "1")]
    json: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    signature: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RevocationDocument {
    sequence: i64,
    issuer_spki_sha256: String,
    revoked_serials: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct VerifiedRevocationArtifact {
    json: Vec<u8>,
    signature: Vec<u8>,
    snapshot: RevocationSnapshot,
    encoded: Vec<u8>,
}

impl VerifiedRevocationArtifact {
    pub(crate) fn snapshot(&self) -> &RevocationSnapshot {
        &self.snapshot
    }

    fn sequence(&self) -> i64 {
        self.snapshot.sequence
    }

    fn same_bytes(&self, other: &Self) -> bool {
        self.json == other.json && self.signature == other.signature
    }

    fn has_canonical_encoding(&self, encoded: &[u8]) -> bool {
        self.encoded.as_slice() == encoded
    }

    fn encode(&self) -> Result<Vec<u8>, RevocationRegistryError> {
        Ok(self.encoded.clone())
    }
}

#[derive(Debug)]
pub(crate) struct RevocationVerifier {
    root_key: PKey<Public>,
    expected_issuer_spki_sha256: [u8; 32],
    dht_key: [u8; 32],
    target: Id,
    revocations: RevocationStore,
    canonical: RwLock<Option<VerifiedRevocationArtifact>>,
}

impl RevocationVerifier {
    pub(crate) fn new(root_ca_pem: &[u8], expected_issuer_spki_sha256: [u8; 32], revocations: RevocationStore) -> Result<Self, RevocationRegistryError> {
        let root_key = (|| {
            let certificates = X509::stack_from_pem(root_ca_pem)?;
            if certificates.len() != 1 {
                return Err(RevocationRegistryError::InvalidRootCa(
                    "expected exactly one root CA certificate".to_string(),
                ));
            }
            let key = certificates[0].public_key()?;
            if key.id() != KeyId::RSA || key.bits() != 4096 {
                return Err(RevocationRegistryError::InvalidRootCa(
                    "root CA must use RSA-4096".to_string(),
                ));
            }
            let actual = hash(MessageDigest::sha256(), &key.public_key_to_der()?)?;
            if actual.as_ref() != expected_issuer_spki_sha256 {
                return Err(RevocationRegistryError::IssuerMismatch {
                    expected: hex_digest(&expected_issuer_spki_sha256),
                    actual: hex_digest(actual.as_ref()),
                });
            }
            Ok(key)
        })()?;
        let signing_key = generate_key(REVOCATION_DHT_KEY)
            .map_err(|error| RevocationRegistryError::InvalidDhtKey(error.to_string()))?;
        let dht_key = signing_key.verifying_key().to_bytes();
        let target = MutableItem::target_from_key(&dht_key, &None);
        Ok(Self {
            root_key,
            expected_issuer_spki_sha256,
            dht_key,
            target,
            revocations,
            canonical: RwLock::new(None),
        })
    }

    pub(crate) fn verify(&self, json: Vec<u8>, signature: Vec<u8>) -> Result<VerifiedRevocationArtifact, RevocationRegistryError> {
        let mut verifier = Verifier::new(MessageDigest::sha256(), &self.root_key)?;
        verifier.set_rsa_padding(Padding::PKCS1_PSS)?;
        verifier.set_rsa_mgf1_md(MessageDigest::sha256())?;
        verifier.set_rsa_pss_saltlen(RsaPssSaltlen::custom(32))?;
        if !verifier
            .verify_oneshot(&signature, &signing_payload(&json))
            .unwrap_or(false)
        {
            return Err(RevocationRegistryError::InvalidSignature);
        }

        let document: RevocationDocument = serde_json::from_slice(&json)
            .map_err(|error| RevocationRegistryError::InvalidDocument(error.to_string()))?;
        if document.sequence < 1 {
            return Err(RevocationRegistryError::InvalidDocument(
                "sequence must be positive".to_string(),
            ));
        }

        let issuer_spki_sha256 = parse_hex::<32>(&document.issuer_spki_sha256, "issuer_spki_sha256")?;
        if issuer_spki_sha256 != self.expected_issuer_spki_sha256 {
            return Err(RevocationRegistryError::IssuerMismatch {
                expected: hex_digest(&self.expected_issuer_spki_sha256),
                actual: hex_digest(&issuer_spki_sha256),
            });
        }

        let mut revoked_serials = HashSet::with_capacity(document.revoked_serials.len());
        for serial in document.revoked_serials {
            let serial = parse_serial(&serial)?;
            if !revoked_serials.insert(serial) {
                return Err(RevocationRegistryError::InvalidDocument(
                    "revoked_serials contains a duplicate".to_string(),
                ));
            }
        }

        let encoded = encode_revocation_record(&json, &signature)?;
        Ok(VerifiedRevocationArtifact {
            json,
            signature,
            snapshot: RevocationSnapshot {
                sequence: document.sequence,
                issuer_spki_sha256,
                revoked_serials,
            },
            encoded,
        })
    }

    fn decode(&self, encoded: &[u8], outer_sequence: Option<i64>) -> Result<VerifiedRevocationArtifact, RevocationRegistryError> {
        let record: SignedRevocationRecord = decode_enveloped_message(encoded)?;
        let artifact = self.verify(record.json, record.signature)?;
        if outer_sequence.is_some_and(|sequence| sequence != artifact.sequence()) {
            return Err(RevocationRegistryError::OuterSequenceMismatch);
        }
        Ok(artifact)
    }

    fn set_canonical(&self, artifact: &VerifiedRevocationArtifact) {
        *self
            .canonical
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(artifact.clone());
    }
}

impl MutableItemValidator for RevocationVerifier {
    fn validate(&self, item: &MutableItem) -> bool {
        if item.target() != &self.target {
            return true;
        }
        if item.key() != &self.dht_key || item.salt().is_some() {
            return false;
        }
        let current_sequence = self.revocations.snapshot().sequence;
        if *item.seq() < current_sequence {
            return false;
        }
        if *item.seq() == current_sequence {
            return self
                .canonical
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .is_some_and(|canonical| canonical.encoded.as_slice() == item.value().as_ref());
        }
        self.decode(item.value(), Some(*item.seq()))
            .is_ok_and(|artifact| {
                artifact.sequence() > current_sequence
                    && artifact.has_canonical_encoding(item.value())
            })
    }

    fn is_protected(&self, item: &MutableItem) -> bool {
        item.target() == &self.target
    }

    fn is_protected_target(&self, target: &Id) -> bool {
        target == &self.target
    }
}

pub(crate) struct DtlsRevocationRegistry {
    verifier: Arc<RevocationVerifier>,
    dht: Arc<DHTMap>,
    state_path: PathBuf,
    current: Mutex<VerifiedRevocationArtifact>,
    advancement_failed: AtomicBool,
    #[cfg(test)]
    fail_after_rename: AtomicBool,
}

impl DtlsRevocationRegistry {
    pub(crate) async fn activate(&self, synchronize: bool) -> Result<(), RevocationRegistryError> {
        for attempt in 0..8 {
            let result = async {
                self.initialize_local().await?;
                if synchronize {
                    self.synchronize().await?;
                }
                self.start_serving().await
            }
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error)
                    if attempt < 7 && error.is_transient() && self.dht.check_security().is_ok() =>
                {
                    let delay = Duration::from_millis(250 * (1 << attempt.min(4)));
                    log::warn!("DTLS startup remains closed; retrying in {delay:?}: {error}");
                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    }

    pub(crate) fn new(verifier: Arc<RevocationVerifier>, dht: Arc<DHTMap>, state_path: PathBuf, initial: VerifiedRevocationArtifact) -> Self {
        verifier.set_canonical(&initial);
        Self {
            verifier,
            dht,
            state_path,
            current: Mutex::new(initial),
            advancement_failed: AtomicBool::new(false),
            #[cfg(test)]
            fail_after_rename: AtomicBool::new(false),
        }
    }

    pub(crate) async fn publish(&self, json: Vec<u8>, signature: Vec<u8>) -> Result<i64, RevocationRegistryError> {
        let incoming = self.verifier.verify(json, signature)?;
        let mut current = self.current.lock().await;
        self.check_advancement()?;
        if incoming.sequence() == current.sequence() && !incoming.same_bytes(&current) {
            return Err(self.sequence_conflict(incoming.sequence()));
        }
        if incoming.sequence() <= current.sequence() {
            return Err(RevocationRegistryError::UnexpectedSequence {
                expected: current.sequence().checked_add(1).unwrap_or(i64::MAX),
                actual: incoming.sequence(),
            });
        }
        self.apply_newer_remote(&mut current).await?;
        if incoming.sequence() == current.sequence() && !incoming.same_bytes(&current) {
            return Err(self.sequence_conflict(incoming.sequence()));
        }

        let expected = current
            .sequence()
            .checked_add(1)
            .ok_or(RevocationRegistryError::SequenceOverflow)?;
        if incoming.sequence() != expected {
            return Err(RevocationRegistryError::UnexpectedSequence {
                expected,
                actual: incoming.sequence(),
            });
        }

        let incoming_encoded = incoming.encode().map_err(|error| self.fatal(error))?;
        let publication = self
            .dht
            .put_raw_mutable(
                REVOCATION_DHT_KEY,
                incoming_encoded,
                incoming.sequence(),
                None,
            )
            .await;
        match self.latest_remote(&current).await? {
            Some(observed) if observed.sequence() == incoming.sequence() => {
                if !observed.same_bytes(&incoming) {
                    return Err(self.sequence_conflict(incoming.sequence()));
                }
            }
            Some(observed) if observed.sequence() > incoming.sequence() => {
                self.persist_and_apply(&observed, &mut current).await?;
                return Err(RevocationRegistryError::UnexpectedSequence {
                    expected: observed.sequence().checked_add(1).unwrap_or(i64::MAX),
                    actual: incoming.sequence(),
                });
            }
            _ => {
                publication?;
                return Err(RevocationRegistryError::PublicationNotConfirmed(
                    incoming.sequence(),
                ));
            }
        }
        self.persist_and_apply(&incoming, &mut current).await?;
        Ok(incoming.sequence())
    }

    pub(crate) async fn synchronize(&self) -> Result<(), RevocationRegistryError> {
        let mut current = self.current.lock().await;
        self.check_advancement()?;
        self.ensure_revocation_enforcement(&current).await?;
        let remote = self.latest_available(&current).await?;
        match remote {
            Some(remote) if remote.sequence() > current.sequence() => {
                self.persist_and_apply(&remote, &mut current).await?;
            }
            Some(remote) if remote.sequence() == current.sequence() => {
                if remote.same_bytes(&current) {
                    self.persist(&remote.encode()?)?;
                } else {
                    return Err(self.sequence_conflict(remote.sequence()));
                }
            }
            _ => {}
        }
        self.ensure_live_enforcement(&current).await?;

        let publication = self
            .dht
            .put_raw_mutable(
                REVOCATION_DHT_KEY,
                current.encode()?,
                current.sequence(),
                None,
            )
            .await;
        let published_sequence = current.sequence();
        let confirmed = match self.latest_remote(&current).await? {
            Some(observed) if observed.sequence() > current.sequence() => {
                self.persist_and_apply(&observed, &mut current).await?;
                true
            }
            Some(observed) if observed.sequence() == current.sequence() => {
                if observed.same_bytes(&current) {
                    true
                } else {
                    return Err(self.sequence_conflict(observed.sequence()));
                }
            }
            _ => false,
        };
        self.apply_newer_local(&mut current).await?;
        if confirmed || current.sequence() > published_sequence {
            Ok(())
        } else {
            publication?;
            Err(RevocationRegistryError::PublicationNotConfirmed(
                published_sequence,
            ))
        }
    }

    pub(crate) async fn initialize_local(&self) -> Result<(), RevocationRegistryError> {
        let mut current = self.current.lock().await;
        self.check_advancement()?;
        self.ensure_revocation_enforcement(&current).await?;
        self.apply_newer_local(&mut current).await?;
        self.ensure_live_enforcement(&current).await
    }

    pub(crate) async fn start_serving(&self) -> Result<(), RevocationRegistryError> {
        let mut current = self.current.lock().await;
        self.check_advancement()?;
        self.apply_newer_local(&mut current).await?;
        self.ensure_live_enforcement(&current).await?;
        if !self
            .dht
            .start_serving_if_protected_matches(
                REVOCATION_DHT_KEY,
                current.encode()?,
                current.sequence(),
            )
            .await?
        {
            return Err(RevocationRegistryError::LocalArtifactNotInstalled(
                current.sequence(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn run_polling(self: Arc<Self>, cancellation_token: CancellationToken, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                _ = ticker.tick() => {
                    if let Err(error) = self.synchronize().await {
                        log::warn!("failed to synchronize DTLS revocations: {error}");
                        if matches!(error, RevocationRegistryError::SequenceConflict(_)) {
                            self.dht.stop_serving();
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn apply_newer_remote(&self, current: &mut VerifiedRevocationArtifact) -> Result<(), RevocationRegistryError> {
        if let Some(remote) = self.latest_available(current).await? {
            if remote.sequence() > current.sequence() {
                self.persist_and_apply(&remote, current).await?;
            } else if remote.sequence() == current.sequence() && !remote.same_bytes(current) {
                return Err(self.sequence_conflict(remote.sequence()));
            }
        }
        Ok(())
    }

    async fn apply_newer_local(&self, current: &mut VerifiedRevocationArtifact) -> Result<(), RevocationRegistryError> {
        let Some(item) = self
            .dht
            .get_local_protected_raw_mutable(REVOCATION_DHT_KEY)
            .await?
        else {
            return Ok(());
        };
        let local = self
            .verifier
            .decode(item.value(), Some(*item.seq()))
            .map_err(|error| self.fatal(error))?;
        if !local.has_canonical_encoding(item.value()) {
            return Err(self.fatal(RevocationRegistryError::InvalidBinaryEncoding));
        }
        if local.sequence() > current.sequence() {
            self.persist_and_apply(&local, current).await?;
        } else if local.sequence() == current.sequence() && !local.same_bytes(current) {
            return Err(self.sequence_conflict(local.sequence()));
        }
        Ok(())
    }

    async fn latest_remote(&self, trusted: &VerifiedRevocationArtifact) -> Result<Option<VerifiedRevocationArtifact>, RevocationRegistryError> {
        self.latest_observed(trusted, false).await
    }

    async fn latest_available(&self, trusted: &VerifiedRevocationArtifact) -> Result<Option<VerifiedRevocationArtifact>, RevocationRegistryError> {
        self.latest_observed(trusted, true).await
    }

    async fn latest_observed(&self, trusted: &VerifiedRevocationArtifact, include_local: bool) -> Result<Option<VerifiedRevocationArtifact>, RevocationRegistryError> {
        let observed = self.dht.get_raw_mutable(REVOCATION_DHT_KEY).await?;
        let local = if include_local {
            self.dht
                .get_local_protected_raw_mutable(REVOCATION_DHT_KEY)
                .await?
        } else {
            None
        };
        self.select_observed(trusted, observed, local)
    }

    fn select_observed(&self, trusted: &VerifiedRevocationArtifact, observed: Vec<MutableItem>,
                       local: Option<MutableItem>) -> Result<Option<VerifiedRevocationArtifact>, RevocationRegistryError> {
        let mut artifacts = Vec::with_capacity(observed.len() + 1);
        for item in observed {
            match self.verifier.decode(item.value(), Some(*item.seq())) {
                Ok(artifact) if artifact.has_canonical_encoding(item.value()) => {
                    artifacts.push(artifact)
                }
                Ok(_) => log::warn!(
                    "discarding noncanonical remote revocation candidate at sequence {}",
                    item.seq()
                ),
                Err(error) => log::warn!(
                    "discarding invalid remote revocation candidate at sequence {}: {error}",
                    item.seq()
                ),
            }
        }
        if let Some(local) = local {
            let artifact = self
                .verifier
                .decode(local.value(), Some(*local.seq()))
                .map_err(|error| self.fatal(error))?;
            if !artifact.has_canonical_encoding(local.value()) {
                return Err(self.fatal(RevocationRegistryError::InvalidBinaryEncoding));
            }
            artifacts.push(artifact);
        }
        match select_latest_remote(artifacts, trusted) {
            Err(RevocationRegistryError::SequenceConflict(sequence)) => {
                Err(self.sequence_conflict(sequence))
            }
            result => result,
        }
    }

    async fn persist_and_apply(&self, artifact: &VerifiedRevocationArtifact, current: &mut VerifiedRevocationArtifact) -> Result<(), RevocationRegistryError> {
        struct AdvancementGuard<'a>(Option<&'a DHTMap>);

        impl Drop for AdvancementGuard<'_> {
            fn drop(&mut self) {
                if let Some(dht) = self.0 {
                    dht.fail_security();
                }
            }
        }

        self.check_advancement()?;
        let encoded = artifact.encode().map_err(|error| self.fatal(error))?;
        self.persist(&encoded)?;
        // Cancellation while awaiting enforcement must fail closed immediately,
        // not just leave a terminal flag for the next registry operation to see.
        self.advancement_failed.store(true, Ordering::Release);
        let mut advancement = AdvancementGuard(Some(&self.dht));
        self.verifier.set_canonical(artifact);
        self.ensure_revocation_enforcement(artifact).await?;
        *current = artifact.clone();
        self.advancement_failed.store(false, Ordering::Release);
        advancement.0 = None;
        self.ensure_local_artifact(artifact).await
    }

    fn check_advancement(&self) -> Result<(), RevocationRegistryError> {
        if self.advancement_failed.load(Ordering::Acquire) {
            Err(self.fatal(RevocationRegistryError::AdvancementFailed))
        } else {
            Ok(())
        }
    }

    fn persist(&self, encoded: &[u8]) -> Result<(), RevocationRegistryError> {
        self.check_advancement()?;
        let result = persist_atomically_with(&self.state_path, encoded, || {
            #[cfg(test)]
            if self.fail_after_rename.swap(false, Ordering::AcqRel) {
                return Err(io::Error::other(
                    "injected parent fsync failure after rename",
                ));
            }
            Ok(())
        });
        result.map_err(|error| {
            // Never interpret an error as proof that rename did not happen.
            self.advancement_failed.store(true, Ordering::Release);
            self.fatal(error)
        })
    }

    async fn ensure_live_enforcement(&self, artifact: &VerifiedRevocationArtifact) -> Result<(), RevocationRegistryError> {
        self.ensure_revocation_enforcement(artifact).await?;
        self.ensure_local_artifact(artifact).await
    }

    async fn ensure_local_artifact(&self, artifact: &VerifiedRevocationArtifact) -> Result<(), RevocationRegistryError> {
        let encoded = artifact.encode().map_err(|error| self.fatal(error))?;
        if !self
            .dht
            .install_protected_raw_mutable(REVOCATION_DHT_KEY, encoded, artifact.sequence())
            .await
            .map_err(|error| self.fatal(error.into()))?
        {
            if self
                .dht
                .get_local_protected_raw_mutable(REVOCATION_DHT_KEY)
                .await?
                .is_some_and(|item| *item.seq() > artifact.sequence())
            {
                return Err(RevocationRegistryError::LocalArtifactNotInstalled(
                    artifact.sequence(),
                ));
            }
            return Err(
                self.fatal(RevocationRegistryError::LocalArtifactNotInstalled(
                    artifact.sequence(),
                )),
            );
        }
        Ok(())
    }

    async fn ensure_revocation_enforcement(&self, artifact: &VerifiedRevocationArtifact) -> Result<(), RevocationRegistryError> {
        if !self
            .dht
            .replace_dtls_revocations(artifact.snapshot.clone())
            .await
            .map_err(|error| self.fatal(error.into()))?
        {
            return Err(self.fatal(RevocationRegistryError::SnapshotNotApplied(
                artifact.sequence(),
            )));
        }
        Ok(())
    }

    fn sequence_conflict(&self, sequence: i64) -> RevocationRegistryError {
        self.fatal(RevocationRegistryError::SequenceConflict(sequence))
    }

    fn fatal(&self, error: RevocationRegistryError) -> RevocationRegistryError {
        self.dht.fail_security();
        error
    }
}

fn select_latest_remote(artifacts: impl IntoIterator<Item = VerifiedRevocationArtifact>,
                        trusted: &VerifiedRevocationArtifact) -> Result<Option<VerifiedRevocationArtifact>, RevocationRegistryError> {
    let mut latest: Option<VerifiedRevocationArtifact> = None;
    let mut relevant_sequences = HashMap::from([(trusted.sequence(), trusted.clone())]);
    for artifact in artifacts {
        if artifact.sequence() >= trusted.sequence() {
            if relevant_sequences
                .get(&artifact.sequence())
                .is_some_and(|observed| !observed.same_bytes(&artifact))
            {
                return Err(RevocationRegistryError::SequenceConflict(
                    artifact.sequence(),
                ));
            }
            relevant_sequences.insert(artifact.sequence(), artifact.clone());
        }
        match &latest {
            Some(current) if current.sequence() > artifact.sequence() => {}
            Some(current) if current.sequence() == artifact.sequence() => {}
            _ => {
                latest = Some(artifact);
            }
        }
    }
    Ok(latest)
}

pub(crate) fn load_initial_artifact(verifier: &RevocationVerifier, state_path: &Path, bootstrap_json_path: &Path,
                                  bootstrap_signature_path: &Path) -> Result<VerifiedRevocationArtifact, RevocationRegistryError> {
    let load_error = |path: &Path, reason: String| RevocationRegistryError::Load {
        path: path.to_path_buf(),
        reason,
    };
    let persisted = match fs::read(state_path) {
        Ok(encoded) => Some(
            verifier
                .decode(&encoded, None)
                .map_err(|error| load_error(state_path, error.to_string()))?,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(load_error(state_path, error.to_string())),
    };
    let bootstrap = match fs::read(bootstrap_json_path) {
        Ok(json) => {
            let bytes = fs::read(bootstrap_signature_path)
                .map_err(|error| load_error(bootstrap_signature_path, error.to_string()))?;
            let signature = parse_detached_signature(&bytes)
                .map_err(|error| load_error(bootstrap_signature_path, error.to_string()))?;
            Some(verifier.verify(json, signature).map_err(|error| {
                load_error(
                    bootstrap_json_path,
                    format!("signature {}: {error}", bootstrap_signature_path.display()),
                )
            })?)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && persisted.is_some() => None,
        Err(error) => return Err(load_error(bootstrap_json_path, error.to_string())),
    };

    let selected = match (persisted, bootstrap) {
        (Some(persisted), Some(bootstrap)) if persisted.sequence() > bootstrap.sequence() => {
            persisted
        }
        (Some(persisted), Some(bootstrap)) if persisted.sequence() == bootstrap.sequence() => {
            if !persisted.same_bytes(&bootstrap) {
                return Err(RevocationRegistryError::SequenceConflict(
                    persisted.sequence(),
                ));
            }
            persisted
        }
        (_, Some(bootstrap)) => bootstrap,
        (Some(persisted), None) => persisted,
        (None, None) => return Err(RevocationRegistryError::MissingInitialArtifact),
    };
    persist_atomically(state_path, &selected.encode()?)?;
    Ok(selected)
}

pub(crate) fn parse_detached_signature(bytes: &[u8]) -> Result<Vec<u8>, RevocationRegistryError> {
    // RSA-4096 signatures are 512 bytes; preserve binary input without trimming.
    if bytes.len() == 512 {
        return Ok(bytes.to_vec());
    }
    let text = std::str::from_utf8(bytes)
        .map(str::trim)
        .map_err(|_| RevocationRegistryError::InvalidSignatureEncoding)?;
    if text.len() == 1024 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return decode_hex(text, "signature");
    }
    for engine in [&STANDARD, &STANDARD_NO_PAD] {
        if let Ok(decoded) = engine.decode(text) {
            if decoded.len() == 512 {
                return Ok(decoded);
            }
        }
    }
    Err(RevocationRegistryError::InvalidSignatureEncoding)
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn parse_serial(value: &str) -> Result<Vec<u8>, RevocationRegistryError> {
    if value.is_empty() || value.len() % 2 != 0 || value.len() > 40 {
        return Err(RevocationRegistryError::InvalidDocument(
            "certificate serial must contain 1 to 20 bytes of hexadecimal data".to_string(),
        ));
    }
    decode_hex(value, "certificate serial")
}

fn parse_hex<const N: usize>(value: &str, field: &'static str) -> Result<[u8; N], RevocationRegistryError> {
    let bytes = decode_hex(value, field)?;
    bytes.try_into().map_err(|_| {
        RevocationRegistryError::InvalidDocument(format!("{field} must contain exactly {N} bytes"))
    })
}

fn decode_hex(value: &str, field: &'static str) -> Result<Vec<u8>, RevocationRegistryError> {
    if !value.is_ascii() || value.len() % 2 != 0 {
        return Err(RevocationRegistryError::InvalidDocument(format!(
            "{field} must contain an even number of hexadecimal characters"
        )));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| {
                RevocationRegistryError::InvalidDocument(format!(
                    "{field} contains non-hexadecimal characters"
                ))
            })
        })
        .collect()
}

fn signing_payload(json: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(SIGNATURE_DOMAIN.len() + json.len());
    payload.extend_from_slice(SIGNATURE_DOMAIN);
    payload.extend_from_slice(json);
    payload
}

fn encode_revocation_record(json: &[u8], signature: &[u8]) -> Result<Vec<u8>, RevocationRegistryError> {
    let encoded = encode_enveloped_message_with_level(
        &SignedRevocationRecord {
            json: json.to_vec(),
            signature: signature.to_vec(),
        },
        REVOCATION_ZSTD_LEVEL,
    )?;
    if encoded.len() > MAX_DHT_VALUE_SIZE {
        return Err(RevocationRegistryError::ValueTooLarge {
            size: encoded.len(),
            max: MAX_DHT_VALUE_SIZE,
        });
    }
    Ok(encoded)
}

fn persist_atomically(path: &Path, value: &[u8]) -> Result<(), RevocationRegistryError> {
    persist_atomically_with(path, value, || Ok(()))
}

fn persist_atomically_with(path: &Path, value: &[u8], after_rename: impl FnOnce() -> io::Result<()>) -> Result<(), RevocationRegistryError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    // Sync every ancestor, including previously created directories from an
    // interrupted attempt, so newly created directory entries are durable.
    let mut directory = parent;
    loop {
        File::open(directory)?.sync_all()?;
        match directory.parent() {
            Some(next) if next.as_os_str().is_empty() => {
                File::open(".")?.sync_all()?;
                break;
            }
            Some(next) => directory = next,
            None => break,
        }
    }
    let temporary_path = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let result = (|| -> Result<(), io::Error> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        file.write_all(value)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)?;
        after_rename()?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.map_err(RevocationRegistryError::from)
}

#[derive(Debug, Error)]
pub(crate) enum RevocationRegistryError {
    #[error(
        "revocation advancement failed; restart required to reconcile persisted high-water state"
    )]
    AdvancementFailed,
    #[error("invalid revocation root CA: {0}")]
    InvalidRootCa(String),
    #[error("OpenSSL revocation operation failed: {0}")]
    Crypto(#[from] openssl::error::ErrorStack),
    #[error("failed to load {path}: {reason}")]
    Load { path: PathBuf, reason: String },
    #[error("invalid reserved DHT key: {0}")]
    InvalidDhtKey(String),
    #[error("invalid binary encoding")]
    InvalidBinaryEncoding,
    #[error("invalid detached signature encoding")]
    InvalidSignatureEncoding,
    #[error("revocation signature is invalid")]
    InvalidSignature,
    #[error("invalid revocation document: {0}")]
    InvalidDocument(String),
    #[error("revocation issuer SPKI SHA256 mismatch: expected {expected}, actual {actual}")]
    IssuerMismatch { expected: String, actual: String },
    #[error("outer BEP44 sequence does not match the signed document")]
    OuterSequenceMismatch,
    #[error("expected revocation sequence {expected}, got {actual}")]
    UnexpectedSequence { expected: i64, actual: i64 },
    #[error("conflicting revocation documents at sequence {0}")]
    SequenceConflict(i64),
    #[error("publication of revocation sequence {0} could not be confirmed")]
    PublicationNotConfirmed(i64),
    #[error("revocation sequence cannot be incremented")]
    SequenceOverflow,
    #[error("encoded revocation registry is too large: {size} bytes, maximum is {max}")]
    ValueTooLarge { size: usize, max: usize },
    #[error("revocation snapshot {0} was not applied")]
    SnapshotNotApplied(i64),
    #[error("revocation artifact {0} was not installed in local DHT storage")]
    LocalArtifactNotInstalled(i64),
    #[error("no signed initial revocation artifact is available")]
    MissingInitialArtifact,
    #[error(transparent)]
    Codec(#[from] DhtCodecError),
    #[error(transparent)]
    DhtGet(#[from] DhtGetError),
    #[error(transparent)]
    DhtPut(#[from] DhtPutError),
    #[error(transparent)]
    Mainline(#[from] mainline::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl RevocationRegistryError {
    fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::PublicationNotConfirmed(_)
                | Self::LocalArtifactNotInstalled(_)
                | Self::DhtPut(DhtPutError::Mainline(mainline::Error::NoClosestNodes))
                | Self::DhtPut(DhtPutError::Mainline(mainline::Error::QueryTimedOut))
                | Self::DhtPut(DhtPutError::Mainline(mainline::Error::PutQueryIsInflight(
                    _
                )))
                | Self::DhtPut(DhtPutError::Mainline(mainline::Error::Dtls(_)))
                | Self::DhtGet(DhtGetError::MainlineDhtError(mainline::Error::Dtls(_)))
        )
    }
}

#[cfg(test)]
mod tests {
    use actix_web::{http::StatusCode, test as actix_test, web, App};
    use mainline::{Bytes, DtlsConfig};
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::ec::{EcGroup, EcKey};
    use openssl::nid::Nid;
    use openssl::pkey::Private;
    use openssl::rsa::Rsa;
    use openssl::sign::Signer;
    use openssl::x509::{extension, X509NameBuilder};
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};
    use std::sync::OnceLock;

    use super::*;
    use crate::controllers::dht_controller::dht_controller;
    use crate::controllers::dtls_revocation_controller;

    #[test]
    fn invalid_remote_candidates_do_not_hide_honest_or_local_advancement() {
        let (dtls, revocations) = test_dtls();
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let initial = verifier.verify(json, signature).unwrap();
        let (_, json, signature) = signed_artifact_at_sequence(2, vec!["01".into()], ca().issuer);
        let newer = verifier.verify(json, signature).unwrap();
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations).unwrap());
        let dht = Arc::new(DHTMap::new(0, &[], dtls, verifier.clone()).unwrap());
        let registry = DtlsRevocationRegistry::new(
            verifier.clone(),
            dht.clone(),
            PathBuf::new(),
            initial.clone(),
        );
        let item = |bytes, sequence| {
            MutableItem::new(
                generate_key(REVOCATION_DHT_KEY).unwrap(),
                Bytes::from(bytes),
                sequence,
                None,
            )
        };
        let honest = item(newer.encode().unwrap(), 2);
        let mut invalid_signature = newer.signature.clone();
        invalid_signature[0] ^= 1;
        let mut noncanonical = newer.encode().unwrap();
        noncanonical.extend_from_slice(&[0x18, 0x01]);
        let invalid = vec![
            item(
                encode_revocation_record(&newer.json, &invalid_signature).unwrap(),
                2,
            ),
            item(noncanonical, 2),
            item(b"malformed".to_vec(), 3),
            item(newer.encode().unwrap(), 99),
        ];
        for position in 0..=invalid.len() {
            let mut remote = invalid.clone();
            remote.insert(position, honest.clone());
            assert_eq!(
                registry
                    .select_observed(&initial, remote, None)
                    .unwrap()
                    .unwrap()
                    .sequence(),
                2
            );
        }
        assert_eq!(
            registry
                .select_observed(&initial, invalid.clone(), Some(honest.clone()))
                .unwrap()
                .unwrap()
                .sequence(),
            2
        );
        assert!(dht.check_security().is_ok());

        let (_, json, signature) = signed_artifact_at_sequence(2, vec!["02".into()], ca().issuer);
        let conflicting = verifier.verify(json, signature).unwrap();
        let mut remote = invalid;
        remote.push(item(conflicting.encode().unwrap(), 2));
        assert!(matches!(
            registry.select_observed(&initial, remote, Some(honest)),
            Err(RevocationRegistryError::SequenceConflict(2))
        ));
        assert!(dht.check_security().is_err());
    }

    #[tokio::test]
    async fn post_rename_failure_cannot_overwrite_disk_high_water_and_restart_adopts_it() {
        let (dtls, revocations) = test_dtls();
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let initial = verifier.verify(json.clone(), signature.clone()).unwrap();
        let (_, newer_json, newer_signature) =
            signed_artifact_at_sequence(2, vec!["01".into()], ca().issuer);
        let newer = verifier.verify(newer_json, newer_signature).unwrap();
        assert!(revocations.replace(initial.snapshot.clone()));
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations.clone()).unwrap());
        let dht = Arc::new(DHTMap::new(0, &[], dtls, verifier.clone()).unwrap());
        let directory =
            std::env::temp_dir().join(format!("tfs-post-rename-{}", rand::random::<u64>()));
        let state = directory.join("state");
        persist_atomically(&state, &initial.encode().unwrap()).unwrap();
        fs::write(directory.join("bootstrap.json"), &json).unwrap();
        fs::write(directory.join("bootstrap.sig"), &signature).unwrap();
        let registry = DtlsRevocationRegistry::new(
            verifier.clone(),
            dht.clone(),
            state.clone(),
            initial.clone(),
        );
        registry.initialize_local().await.unwrap();
        registry.fail_after_rename.store(true, Ordering::Release);
        {
            let mut current = registry.current.lock().await;
            assert!(matches!(
                registry.persist_and_apply(&newer, &mut current).await,
                Err(RevocationRegistryError::Io(_))
            ));
            assert_eq!(current.sequence(), 1);
        }
        assert_eq!(revocations.snapshot().sequence, 1);
        assert_eq!(
            verifier
                .decode(&fs::read(&state).unwrap(), None)
                .unwrap()
                .sequence(),
            2
        );
        assert!(matches!(
            registry.synchronize().await,
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert!(matches!(
            registry.publish(json, signature).await,
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert!(matches!(
            registry.initialize_local().await,
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert!(matches!(
            registry.start_serving().await,
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert!(registry.persist(&initial.encode().unwrap()).is_err());
        assert_eq!(fs::read(&state).unwrap(), newer.encode().unwrap());
        assert!(dht.check_security().is_err());

        let restarted_verifier =
            RevocationVerifier::new(&ca().pem, ca().issuer, RevocationStore::default()).unwrap();
        let restarted = load_initial_artifact(
            &restarted_verifier,
            &state,
            &directory.join("bootstrap.json"),
            &directory.join("bootstrap.sig"),
        )
        .unwrap();
        assert_eq!(restarted.sequence(), 2);
        assert_eq!(fs::read(&state).unwrap(), newer.encode().unwrap());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn cancellation_during_actor_enforcement_immediately_fails_closed() {
        #[derive(Debug)]
        struct BlockingValidator {
            inner: Arc<RevocationVerifier>,
            block: AtomicBool,
            entered: flume::Sender<()>,
            release: flume::Receiver<()>,
        }

        impl MutableItemValidator for BlockingValidator {
            fn validate(&self, item: &MutableItem) -> bool {
                if self.block.swap(false, Ordering::AcqRel) {
                    self.entered.send(()).unwrap();
                    self.release.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                self.inner.validate(item)
            }

            fn is_protected(&self, item: &MutableItem) -> bool {
                self.inner.is_protected(item)
            }

            fn is_protected_target(&self, target: &Id) -> bool {
                self.inner.is_protected_target(target)
            }
        }

        let (dtls, revocations) = test_dtls();
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let initial = verifier.verify(json, signature).unwrap();
        let (_, json, signature) = signed_artifact_at_sequence(2, vec!["01".into()], ca().issuer);
        let newer = verifier.verify(json, signature).unwrap();
        assert!(revocations.replace(initial.snapshot.clone()));
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations.clone()).unwrap());
        let (entered_sender, entered) = flume::bounded(1);
        let (release, release_receiver) = flume::bounded(1);
        let validator = Arc::new(BlockingValidator {
            inner: verifier.clone(),
            block: AtomicBool::new(false),
            entered: entered_sender,
            release: release_receiver,
        });
        let dht = Arc::new(DHTMap::new(0, &[], dtls, validator.clone()).unwrap());
        let directory =
            std::env::temp_dir().join(format!("tfs-cancel-advancement-{}", rand::random::<u64>()));
        let state = directory.join("state");
        persist_atomically(&state, &initial.encode().unwrap()).unwrap();
        let registry = Arc::new(DtlsRevocationRegistry::new(
            verifier,
            dht.clone(),
            state.clone(),
            initial.clone(),
        ));
        registry.initialize_local().await.unwrap();
        registry.start_serving().await.unwrap();

        // Occupy the actual actor before queuing snapshot enforcement. This
        // guarantees advancement is suspended at its enforcement await.
        validator.block.store(true, Ordering::Release);
        let mut blocker = Box::pin(dht.install_protected_raw_mutable(
            REVOCATION_DHT_KEY,
            initial.encode().unwrap(),
            1,
        ));
        assert!(futures::poll!(&mut blocker).is_pending());
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut advancement = Box::pin(async {
            let mut current = registry.current.lock().await;
            registry.persist_and_apply(&newer, &mut current).await
        });
        assert!(futures::poll!(&mut advancement).is_pending());
        assert_eq!(fs::read(&state).unwrap(), newer.encode().unwrap());
        assert_eq!(revocations.snapshot().sequence, 1);
        assert!(registry.advancement_failed.load(Ordering::Acquire));
        assert!(dht.is_serving());
        assert!(!revocations.security_failed());

        drop(advancement);

        // No subsequent registry call is needed to close application serving.
        assert!(revocations.security_failed());
        assert!(!dht.is_serving());
        assert!(registry.advancement_failed.load(Ordering::Acquire));
        assert_eq!(registry.current.lock().await.sequence(), 1);
        assert!(matches!(
            dht.get_value("application").await,
            Err(DhtGetError::MainlineDhtError(
                mainline::Error::SecurityFailed
            ))
        ));
        assert!(matches!(
            dht.put("application", &"value".to_string()).await,
            Err(DhtPutError::Mainline(mainline::Error::SecurityFailed))
        ));
        assert!(matches!(
            registry.synchronize().await,
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert!(matches!(
            registry.persist(&initial.encode().unwrap()),
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert_eq!(fs::read(&state).unwrap(), newer.encode().unwrap());

        release.send(()).unwrap();
        blocker.await.unwrap();
        // Drain the queued enforcement through an actor barrier. Its late
        // completion cannot clear either latch or advance registry memory.
        dht.local_addr().await.unwrap();
        assert_eq!(revocations.snapshot().sequence, 2);
        assert!(revocations.security_failed());
        assert!(registry.advancement_failed.load(Ordering::Acquire));
        assert_eq!(registry.current.lock().await.sequence(), 1);
        assert!(matches!(
            registry.synchronize().await,
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert_eq!(fs::read(&state).unwrap(), newer.encode().unwrap());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn observing_terminal_advancement_latches_global_failure() {
        let (dtls, revocations) = test_dtls();
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let initial = verifier.verify(json, signature).unwrap();
        assert!(revocations.replace(initial.snapshot.clone()));
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations.clone()).unwrap());
        let dht = Arc::new(DHTMap::new(0, &[], dtls, verifier.clone()).unwrap());
        let registry = DtlsRevocationRegistry::new(verifier, dht.clone(), PathBuf::new(), initial);
        registry.initialize_local().await.unwrap();
        registry.start_serving().await.unwrap();
        registry.advancement_failed.store(true, Ordering::Release);
        assert!(dht.is_serving());
        assert!(!revocations.security_failed());
        assert!(matches!(
            registry.synchronize().await,
            Err(RevocationRegistryError::AdvancementFailed)
        ));
        assert!(revocations.security_failed());
        assert!(!dht.is_serving());
        assert!(registry.advancement_failed.load(Ordering::Acquire));
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02X}")).collect()
    }

    struct TestCa {
        key: PKey<Private>,
        pem: Vec<u8>,
        issuer: [u8; 32],
        leaf: Vec<u8>,
        leaf_key: Vec<u8>,
    }

    fn ca() -> &'static TestCa {
        static CA: OnceLock<TestCa> = OnceLock::new();
        CA.get_or_init(|| {
            let key = PKey::from_rsa(Rsa::generate(4096).unwrap()).unwrap();
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", "TFS test root").unwrap();
            let name = name.build();
            let mut root = X509::builder().unwrap();
            root.set_version(2).unwrap();
            root.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
                .unwrap();
            root.set_subject_name(&name).unwrap();
            root.set_issuer_name(&name).unwrap();
            root.set_pubkey(&key).unwrap();
            root.set_not_before(&Asn1Time::days_from_now(0).unwrap())
                .unwrap();
            root.set_not_after(&Asn1Time::days_from_now(30).unwrap())
                .unwrap();
            root.append_extension(
                extension::BasicConstraints::new()
                    .critical()
                    .ca()
                    .build()
                    .unwrap(),
            )
            .unwrap();
            root.append_extension(
                extension::KeyUsage::new()
                    .critical()
                    .key_cert_sign()
                    .crl_sign()
                    .build()
                    .unwrap(),
            )
            .unwrap();
            root.sign(&key, MessageDigest::sha256()).unwrap();
            let root = root.build();
            let leaf_key = PKey::from_ec_key(
                EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
            )
            .unwrap();
            let mut leaf = X509::builder().unwrap();
            leaf.set_version(2).unwrap();
            leaf.set_serial_number(&BigNum::from_u32(2).unwrap().to_asn1_integer().unwrap())
                .unwrap();
            let mut leaf_name = X509NameBuilder::new().unwrap();
            leaf_name
                .append_entry_by_text("CN", "TFS test node")
                .unwrap();
            leaf.set_subject_name(&leaf_name.build()).unwrap();
            leaf.set_issuer_name(&name).unwrap();
            leaf.set_pubkey(&leaf_key).unwrap();
            leaf.set_not_before(&Asn1Time::days_from_now(0).unwrap())
                .unwrap();
            leaf.set_not_after(&Asn1Time::days_from_now(30).unwrap())
                .unwrap();
            leaf.append_extension(
                extension::BasicConstraints::new()
                    .critical()
                    .build()
                    .unwrap(),
            )
            .unwrap();
            leaf.append_extension(
                extension::KeyUsage::new()
                    .critical()
                    .digital_signature()
                    .build()
                    .unwrap(),
            )
            .unwrap();
            leaf.append_extension(
                extension::ExtendedKeyUsage::new()
                    .client_auth()
                    .server_auth()
                    .build()
                    .unwrap(),
            )
            .unwrap();
            let san = extension::SubjectAlternativeName::new()
                .dns("dht.tfs.internal")
                .build(&leaf.x509v3_context(Some(&root), None))
                .unwrap();
            leaf.append_extension(san).unwrap();
            leaf.sign(&key, MessageDigest::sha256()).unwrap();
            TestCa {
                issuer: hash(MessageDigest::sha256(), &key.public_key_to_der().unwrap())
                    .unwrap()
                    .as_ref()
                    .try_into()
                    .unwrap(),
                pem: root.to_pem().unwrap(),
                key,
                leaf: leaf.build().to_pem().unwrap(),
                leaf_key: leaf_key.private_key_to_pem_pkcs8().unwrap(),
            }
        })
    }

    fn test_dtls() -> (DtlsConfig, RevocationStore) {
        let revocations = RevocationStore::default();
        let dtls = DtlsConfig::from_pem(&ca().leaf, &ca().leaf_key, &ca().pem, revocations.clone())
            .unwrap();
        (dtls, revocations)
    }

    fn sign(json: &[u8]) -> Vec<u8> {
        let mut signer = Signer::new(MessageDigest::sha256(), &ca().key).unwrap();
        signer.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
        signer.set_rsa_mgf1_md(MessageDigest::sha256()).unwrap();
        signer
            .set_rsa_pss_saltlen(RsaPssSaltlen::custom(32))
            .unwrap();
        signer.sign_oneshot_to_vec(&signing_payload(json)).unwrap()
    }

    fn signed_artifact(serials: Vec<String>) -> (RevocationVerifier, Vec<u8>, Vec<u8>) {
        signed_artifact_for_issuer(serials, ca().issuer)
    }

    fn signed_artifact_for_issuer(serials: Vec<String>, issuer: [u8; 32]) -> (RevocationVerifier, Vec<u8>, Vec<u8>) {
        signed_artifact_at_sequence(1, serials, issuer)
    }

    fn signed_artifact_at_sequence(sequence: i64, serials: Vec<String>, issuer: [u8; 32]) -> (RevocationVerifier, Vec<u8>, Vec<u8>) {
        let verifier =
            RevocationVerifier::new(&ca().pem, ca().issuer, RevocationStore::default()).unwrap();
        let json = serde_json::to_vec(&RevocationDocument {
            sequence,
            issuer_spki_sha256: hex(&issuer),
            revoked_serials: serials,
        })
        .unwrap();
        let signature = sign(&json);
        (verifier, json, signature)
    }

    #[test]
    fn verifies_exact_signed_json() {
        let (verifier, json, signature) = signed_artifact(vec!["01AF".to_string()]);
        let artifact = verifier.verify(json.clone(), signature.clone()).unwrap();
        assert!(artifact.snapshot.revoked_serials.contains(&vec![1, 175]));

        let mut tampered = json;
        tampered.push(b' ');
        assert!(matches!(
            verifier.verify(tampered, signature),
            Err(RevocationRegistryError::InvalidSignature)
        ));
    }

    #[test]
    fn rejects_other_signature_parameters_keys_and_missing_domain() {
        let (verifier, json, _) = signed_artifact(Vec::new());
        let other_key = PKey::from_rsa(Rsa::generate(4096).unwrap()).unwrap();
        for (key, padding, digest, mgf, salt, payload) in [
            (
                &ca().key,
                Padding::PKCS1,
                MessageDigest::sha256(),
                MessageDigest::sha256(),
                32,
                signing_payload(&json),
            ),
            (
                &ca().key,
                Padding::PKCS1_PSS,
                MessageDigest::sha384(),
                MessageDigest::sha256(),
                32,
                signing_payload(&json),
            ),
            (
                &ca().key,
                Padding::PKCS1_PSS,
                MessageDigest::sha256(),
                MessageDigest::sha384(),
                32,
                signing_payload(&json),
            ),
            (
                &ca().key,
                Padding::PKCS1_PSS,
                MessageDigest::sha256(),
                MessageDigest::sha256(),
                0,
                signing_payload(&json),
            ),
            (
                &ca().key,
                Padding::PKCS1_PSS,
                MessageDigest::sha256(),
                MessageDigest::sha256(),
                64,
                signing_payload(&json),
            ),
            (
                &ca().key,
                Padding::PKCS1_PSS,
                MessageDigest::sha256(),
                MessageDigest::sha256(),
                32,
                json.clone(),
            ),
            (
                &other_key,
                Padding::PKCS1_PSS,
                MessageDigest::sha256(),
                MessageDigest::sha256(),
                32,
                signing_payload(&json),
            ),
        ] {
            let mut signer = Signer::new(digest, key).unwrap();
            signer.set_rsa_padding(padding).unwrap();
            if padding == Padding::PKCS1_PSS {
                signer.set_rsa_mgf1_md(mgf).unwrap();
                signer
                    .set_rsa_pss_saltlen(RsaPssSaltlen::custom(salt))
                    .unwrap();
            }
            let signature = signer.sign_oneshot_to_vec(&payload).unwrap();
            assert!(matches!(
                verifier.verify(json.clone(), signature),
                Err(RevocationRegistryError::InvalidSignature)
            ));
        }
        for signature in [
            vec![],
            vec![0; 64],
            vec![0; 511],
            vec![0; 512],
            vec![0; 513],
        ] {
            assert!(matches!(
                verifier.verify(json.clone(), signature),
                Err(RevocationRegistryError::InvalidSignature)
            ));
        }
    }

    #[test]
    fn validates_root_certificate_and_reports_issuer_digests() {
        for pem in [
            Vec::new(),
            b"not a certificate".to_vec(),
            ca().leaf.clone(),
            [ca().pem.clone(), ca().pem.clone()].concat(),
        ] {
            assert!(
                RevocationVerifier::new(&pem, ca().issuer, RevocationStore::default()).is_err()
            );
        }
        let error =
            RevocationVerifier::new(&ca().pem, [42; 32], RevocationStore::default()).unwrap_err();
        assert!(error.to_string().contains(&format!(
            "expected {}, actual {}",
            hex(&[42; 32]),
            hex(&ca().issuer)
        )));
        let (verifier, json, signature) = signed_artifact_for_issuer(Vec::new(), [42; 32]);
        let error = verifier.verify(json, signature).unwrap_err();
        assert!(error.to_string().contains(&format!(
            "expected {}, actual {}",
            hex(&ca().issuer),
            hex(&[42; 32])
        )));
    }

    #[test]
    fn detached_signature_encodings_round_trip() {
        let (_, _, signature) = signed_artifact(Vec::new());
        for bytes in [
            signature.clone(),
            hex(&signature).into_bytes(),
            STANDARD.encode(&signature).into_bytes(),
            STANDARD_NO_PAD.encode(&signature).into_bytes(),
            format!(" {}\n", STANDARD.encode(&signature)).into_bytes(),
        ] {
            assert_eq!(parse_detached_signature(&bytes).unwrap(), signature);
        }
        for bytes in [vec![], vec![0; 64], vec![0; 511], b"invalid".to_vec()] {
            assert!(parse_detached_signature(&bytes).is_err());
        }
    }

    #[test]
    fn rejects_oversized_signed_registry() {
        let mut random = StdRng::seed_from_u64(7);
        let serials = (0..32)
            .map(|_| {
                let mut serial = [0; 20];
                random.fill_bytes(&mut serial);
                hex(&serial)
            })
            .collect();
        let (verifier, json, signature) = signed_artifact(serials);
        assert!(matches!(
            verifier.verify(json, signature),
            Err(RevocationRegistryError::ValueTooLarge { max: 1000, .. })
        ));
    }

    #[test]
    fn openssl_cli_signature_is_accepted() {
        let (verifier, json, _) = signed_artifact(Vec::new());
        let directory =
            std::env::temp_dir().join(format!("tfs-revocations-openssl-{}", rand::random::<u64>()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("root.key"),
            ca().key.private_key_to_pem_pkcs8().unwrap(),
        )
        .unwrap();
        fs::write(directory.join("payload"), signing_payload(&json)).unwrap();
        let output = std::process::Command::new("openssl")
            .current_dir(&directory)
            .args([
                "dgst",
                "-sha256",
                "-sign",
                "root.key",
                "-sigopt",
                "rsa_padding_mode:pss",
                "-sigopt",
                "rsa_mgf1_md:sha256",
                "-sigopt",
                "rsa_pss_saltlen:32",
                "-out",
                "signature",
                "payload",
            ])
            .output()
            .expect("OpenSSL CLI must be installed for the signing interoperability test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        verifier
            .verify(json, fs::read(directory.join("signature")).unwrap())
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn load_errors_identify_artifact_paths() {
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let directory =
            std::env::temp_dir().join(format!("tfs-revocations-errors-{}", rand::random::<u64>()));
        fs::create_dir_all(&directory).unwrap();
        let state = directory.join("state");
        let bootstrap = directory.join("bootstrap.json");
        let sig = directory.join("bootstrap.sig");
        let error = load_initial_artifact(&verifier, &state, &bootstrap, &sig).unwrap_err();
        assert!(error.to_string().contains(bootstrap.to_str().unwrap()));
        fs::write(&bootstrap, json).unwrap();
        let error = load_initial_artifact(&verifier, &state, &bootstrap, &sig).unwrap_err();
        assert!(error.to_string().contains(sig.to_str().unwrap()));
        fs::write(&sig, b"invalid").unwrap();
        let error = load_initial_artifact(&verifier, &state, &bootstrap, &sig).unwrap_err();
        assert!(error.to_string().contains(sig.to_str().unwrap()));
        fs::write(&sig, signature).unwrap();
        fs::write(&state, b"invalid").unwrap();
        let error = load_initial_artifact(&verifier, &state, &bootstrap, &sig).unwrap_err();
        assert!(error.to_string().contains(state.to_str().unwrap()));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_outer_sequence_mismatch() {
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let encoded = verifier.verify(json, signature).unwrap().encode().unwrap();
        assert!(matches!(
            verifier.decode(&encoded, Some(2)),
            Err(RevocationRegistryError::OuterSequenceMismatch)
        ));
    }

    #[test]
    fn five_random_serials_fit_one_dht_value() {
        for seed in 0..16 {
            let mut random = StdRng::seed_from_u64(seed);
            let issuer = ca().issuer;
            let serials = (0..5)
                .map(|_| {
                    let mut serial = [0u8; 20];
                    random.fill_bytes(&mut serial);
                    hex(&serial)
                })
                .collect();
            let (verifier, json, signature) = signed_artifact_for_issuer(serials, issuer);
            let encoded = verifier.verify(json, signature).unwrap().encode().unwrap();
            assert!(
                encoded.len() <= MAX_DHT_VALUE_SIZE,
                "seed={seed}, size={}",
                encoded.len()
            );
        }
    }

    #[test]
    fn reserved_target_requires_valid_inner_signature_and_matching_sequence() {
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let artifact = verifier.verify(json, signature).unwrap();
        let encoded = artifact.encode().unwrap();
        let mut noncanonical_encoded = encoded.clone();
        noncanonical_encoded.extend_from_slice(&[0x18, 0x01]);
        assert!(verifier.decode(&noncanonical_encoded, Some(1)).is_ok());

        let signing_key = generate_key(REVOCATION_DHT_KEY).unwrap();
        let valid = MutableItem::new(signing_key, Bytes::from(encoded), 1, None);
        assert!(verifier.validate(&valid));

        let noncanonical = MutableItem::new(
            generate_key(REVOCATION_DHT_KEY).unwrap(),
            Bytes::from(noncanonical_encoded),
            1,
            None,
        );
        assert!(!verifier.validate(&noncanonical));

        verifier.set_canonical(&artifact);
        assert!(verifier.revocations.replace(artifact.snapshot.clone()));
        assert!(verifier.validate(&valid));
        assert!(!verifier.validate(&noncanonical));

        assert!(verifier.revocations.replace(RevocationSnapshot {
            sequence: 2,
            issuer_spki_sha256: ca().issuer,
            revoked_serials: HashSet::new(),
        }));
        assert!(!verifier.validate(&valid));

        let signing_key = generate_key(REVOCATION_DHT_KEY).unwrap();
        let invalid = MutableItem::new(signing_key, Bytes::from_static(b"invalid"), 1, None);
        assert!(!verifier.validate(&invalid));

        let unrelated = MutableItem::new(
            generate_key("unrelated").unwrap(),
            Bytes::from_static(b"application data"),
            1,
            None,
        );
        assert!(verifier.validate(&unrelated));
    }

    #[test]
    fn ignores_conflicting_remote_history_below_trusted_sequence() {
        let issuer = ca().issuer;
        let (verifier, first_json, first_signature) =
            signed_artifact_at_sequence(2, Vec::new(), issuer);
        let (_, second_json, second_signature) =
            signed_artifact_at_sequence(2, vec!["01AF".to_string()], issuer);
        let first = verifier.verify(first_json, first_signature).unwrap();
        let second = verifier.verify(second_json, second_signature).unwrap();
        let (_, newer_json, newer_signature) =
            signed_artifact_at_sequence(3, vec!["02".to_string()], issuer);
        let newer = verifier.verify(newer_json, newer_signature).unwrap();

        assert!(matches!(
            select_latest_remote([first.clone(), second.clone()], &first),
            Err(RevocationRegistryError::SequenceConflict(2))
        ));
        assert!(matches!(
            select_latest_remote([second.clone(), newer.clone()], &first),
            Err(RevocationRegistryError::SequenceConflict(2))
        ));
        assert!(matches!(
            select_latest_remote([newer.clone(), second.clone(), first.clone()], &first),
            Err(RevocationRegistryError::SequenceConflict(2))
        ));
        assert_eq!(
            select_latest_remote([first, second, newer.clone()], &newer)
                .unwrap()
                .unwrap()
                .sequence(),
            3
        );
    }

    #[test]
    fn reserved_target_rejects_conflicting_current_artifact() {
        let issuer = ca().issuer;
        let (verifier, current_json, current_signature) =
            signed_artifact_at_sequence(1, Vec::new(), issuer);
        let current = verifier.verify(current_json, current_signature).unwrap();
        verifier.set_canonical(&current);
        assert!(verifier.revocations.replace(current.snapshot.clone()));

        let (_, conflicting_json, conflicting_signature) =
            signed_artifact_at_sequence(1, vec!["01AF".to_string()], issuer);
        let conflicting = verifier
            .verify(conflicting_json, conflicting_signature)
            .unwrap();
        let item = MutableItem::new(
            generate_key(REVOCATION_DHT_KEY).unwrap(),
            Bytes::from(conflicting.encode().unwrap()),
            1,
            None,
        );

        assert!(!verifier.validate(&item));
    }

    #[test]
    fn startup_selects_and_persists_highest_local_sequence() {
        let issuer = ca().issuer;
        let (verifier, old_json, old_signature) =
            signed_artifact_at_sequence(1, Vec::new(), issuer);
        let old_artifact = verifier.verify(old_json, old_signature).unwrap();
        let (_, new_json, new_signature) =
            signed_artifact_at_sequence(2, vec!["01AF".to_string()], issuer);
        let directory = std::env::temp_dir().join(format!(
            "tfs-revocations-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(&directory).unwrap();
        let state_path = directory.join("state");
        let json_path = directory.join("bootstrap.json");
        let signature_path = directory.join("bootstrap.sig");
        fs::write(&state_path, old_artifact.encode().unwrap()).unwrap();
        fs::write(&json_path, new_json).unwrap();
        fs::write(&signature_path, new_signature).unwrap();

        let selected =
            load_initial_artifact(&verifier, &state_path, &json_path, &signature_path).unwrap();
        assert_eq!(selected.sequence(), 2);
        assert_eq!(
            verifier
                .decode(&fs::read(&state_path).unwrap(), None)
                .unwrap()
                .sequence(),
            2
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn initialization_adopts_newer_protected_local_artifact() {
        let (dtls, revocations) = test_dtls();
        let issuer = dtls.issuer_spki_sha256();
        let (verifier, initial_json, initial_signature) =
            signed_artifact_at_sequence(1, Vec::new(), issuer);
        let initial = verifier.verify(initial_json, initial_signature).unwrap();
        let (_, newer_json, newer_signature) =
            signed_artifact_at_sequence(2, vec!["01AF".to_string()], issuer);
        let newer = verifier.verify(newer_json, newer_signature).unwrap();
        assert!(revocations.replace(initial.snapshot.clone()));
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, issuer, revocations.clone()).unwrap());
        verifier.set_canonical(&initial);
        let dht = Arc::new(DHTMap::new(0, &[], dtls, verifier.clone()).unwrap());
        assert!(dht
            .install_protected_raw_mutable(
                REVOCATION_DHT_KEY,
                newer.encode().unwrap(),
                newer.sequence(),
            )
            .await
            .unwrap());
        assert!(!dht
            .install_protected_raw_mutable(
                REVOCATION_DHT_KEY,
                initial.encode().unwrap(),
                initial.sequence(),
            )
            .await
            .unwrap());
        assert!(!dht
            .start_serving_if_protected_matches(
                REVOCATION_DHT_KEY,
                initial.encode().unwrap(),
                initial.sequence(),
            )
            .await
            .unwrap());
        let directory = std::env::temp_dir().join(format!(
            "tfs-revocations-local-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(&directory).unwrap();
        let state_path = directory.join("state");
        let registry =
            DtlsRevocationRegistry::new(verifier.clone(), dht.clone(), state_path.clone(), initial);

        registry.initialize_local().await.unwrap();
        registry.start_serving().await.unwrap();

        assert_eq!(revocations.snapshot(), newer.snapshot);
        assert_eq!(
            verifier
                .decode(&fs::read(&state_path).unwrap(), None)
                .unwrap()
                .sequence(),
            2
        );
        assert_eq!(
            dht.get_local_protected_raw_mutable(REVOCATION_DHT_KEY)
                .await
                .unwrap()
                .unwrap()
                .seq(),
            &2
        );
        assert!(dht.is_serving());

        let (_, conflicting_json, conflicting_signature) =
            signed_artifact_at_sequence(2, vec!["02".to_string()], issuer);
        assert!(matches!(
            registry
                .publish(conflicting_json, conflicting_signature)
                .await,
            Err(RevocationRegistryError::SequenceConflict(2))
        ));
        assert!(!dht.is_serving());

        drop(registry);
        drop(dht);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_persistence_handles_tmp_state_filename() {
        let directory = std::env::temp_dir().join(format!(
            "tfs-revocations-tmp-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(&directory).unwrap();
        let state_path = directory.join("state.tmp");

        persist_atomically(&state_path, b"first").unwrap();
        persist_atomically(&state_path, b"second").unwrap();
        assert_eq!(fs::read(&state_path).unwrap(), b"second");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_persistence_handles_bare_relative_and_new_nested_directories() {
        let name = format!("tfs-revocation-state-{}", rand::random::<u64>());
        let path = Path::new(&name);
        persist_atomically(path, b"first").unwrap();
        persist_atomically(path, b"second").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"second");
        fs::remove_file(path).unwrap();
        let directory = std::env::temp_dir().join(name);
        let path = directory.join("new/nested/state");
        persist_atomically(&path, b"nested").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"nested");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn startup_retry_classification_excludes_security_and_persistence_errors() {
        for error in [
            RevocationRegistryError::InvalidSignature,
            RevocationRegistryError::SequenceConflict(2),
            RevocationRegistryError::InvalidBinaryEncoding,
            RevocationRegistryError::Io(io::Error::other("disk failure")),
        ] {
            assert!(!error.is_transient());
        }
        assert!(RevocationRegistryError::PublicationNotConfirmed(1).is_transient());
        assert!(RevocationRegistryError::LocalArtifactNotInstalled(1).is_transient());
        assert!(RevocationRegistryError::DhtPut(DhtPutError::Mainline(
            mainline::Error::NoClosestNodes
        ))
        .is_transient());
    }

    #[tokio::test]
    async fn persistence_failure_does_not_apply_newer_revocations() {
        let (dtls, revocations) = test_dtls();
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let initial = verifier.verify(json, signature).unwrap();
        assert!(revocations.replace(initial.snapshot.clone()));
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations.clone()).unwrap());
        verifier.set_canonical(&initial);
        let dht = Arc::new(DHTMap::new(0, &[], dtls, verifier.clone()).unwrap());
        let directory = std::env::temp_dir().join(format!(
            "tfs-revocations-persist-failure-{}",
            rand::random::<u64>()
        ));
        fs::create_dir_all(&directory).unwrap();
        let registry = DtlsRevocationRegistry::new(
            verifier.clone(),
            dht.clone(),
            directory.clone(),
            initial.clone(),
        );
        registry.initialize_local().await.unwrap();
        registry.start_serving().await.unwrap();
        let (_, json, signature) =
            signed_artifact_at_sequence(2, vec!["01AF".to_string()], ca().issuer);
        let newer = verifier.verify(json, signature).unwrap();
        let mut current = registry.current.lock().await;
        // Renaming a snapshot file over a directory fails, even when tests run as root.
        assert!(registry
            .persist_and_apply(&newer, &mut current)
            .await
            .is_err());
        assert_eq!(current.sequence(), 1);
        assert_eq!(revocations.snapshot(), initial.snapshot);
        assert_eq!(
            verifier
                .canonical
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .sequence(),
            1
        );
        assert!(!dht.is_serving());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn fatal_failure_rejects_inflight_application_results_and_keeps_recovery() {
        let (dtls, revocations) = test_dtls();
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let initial = verifier.verify(json, signature).unwrap();
        revocations.replace(initial.snapshot.clone());
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations).unwrap());
        verifier.set_canonical(&initial);
        let dht = Arc::new(DHTMap::new(0, &[], dtls, verifier).unwrap());
        let pending = dht.get_value("application");
        tokio::pin!(pending);
        assert!(futures::poll!(&mut pending).is_pending());
        dht.fail_security();
        assert!(matches!(
            pending.await,
            Err(DhtGetError::MainlineDhtError(
                mainline::Error::SecurityFailed
            ))
        ));
        assert!(matches!(
            dht.put("application", &"value".to_string()).await,
            Err(DhtPutError::Mainline(mainline::Error::SecurityFailed))
        ));
        assert!(dht
            .install_protected_raw_mutable(REVOCATION_DHT_KEY, initial.encode().unwrap(), 1)
            .await
            .unwrap());
        assert!(dht
            .get_local_protected_raw_mutable(REVOCATION_DHT_KEY)
            .await
            .unwrap()
            .is_some());
        assert!(dht
            .start_serving_if_protected_matches(REVOCATION_DHT_KEY, initial.encode().unwrap(), 1)
            .await
            .is_err());
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(dht.clone()))
                .service(dht_controller::config()),
        )
        .await;
        for request in [
            actix_test::TestRequest::get()
                .uri("/dht/application")
                .to_request(),
            actix_test::TestRequest::post()
                .uri("/dht/application")
                .set_payload("value")
                .to_request(),
        ] {
            assert_eq!(
                actix_test::call_service(&app, request).await.status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }

    #[tokio::test]
    async fn publishes_registry_through_local_dtls_dht() {
        let (dtls, revocations) = test_dtls();
        let issuer = dtls.issuer_spki_sha256();
        let (verifier, initial_json, initial_signature) =
            signed_artifact_at_sequence(1, Vec::new(), issuer);
        let initial = verifier.verify(initial_json, initial_signature).unwrap();
        assert!(revocations.replace(initial.snapshot.clone()));
        let verifier =
            Arc::new(RevocationVerifier::new(&ca().pem, issuer, revocations.clone()).unwrap());
        verifier.set_canonical(&initial);
        let storage_dht = Arc::new(DHTMap::new(0, &[], dtls.clone(), verifier.clone()).unwrap());
        assert!(storage_dht
            .install_protected_raw_mutable(
                REVOCATION_DHT_KEY,
                initial.encode().unwrap(),
                initial.sequence(),
            )
            .await
            .unwrap());
        let storage_address = storage_dht.local_addr().await.unwrap();
        let dht = Arc::new(
            DHTMap::new(
                0,
                &[format!("127.0.0.1:{}", storage_address.port())],
                dtls.clone(),
                verifier.clone(),
            )
            .unwrap(),
        );
        let directory = std::env::temp_dir().join(format!(
            "tfs-revocations-network-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(&directory).unwrap();
        let state_path = directory.join("state");
        let registry = Arc::new(DtlsRevocationRegistry::new(
            verifier.clone(),
            dht.clone(),
            state_path.clone(),
            initial,
        ));

        tokio::time::timeout(Duration::from_secs(20), registry.synchronize())
            .await
            .unwrap()
            .unwrap();
        let (_, json, signature) = signed_artifact_at_sequence(2, vec!["01AF".to_string()], issuer);
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(registry.clone()))
                .app_data(web::Data::new(dht.clone()))
                .service(dtls_revocation_controller::config())
                .service(dht_controller::config()),
        )
        .await;
        let reserved_put = actix_test::TestRequest::post()
            .uri("/dht/__tfs/security/dtls/revocations")
            .set_payload("unsigned")
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, reserved_put).await.status(),
            StatusCode::FORBIDDEN
        );
        let request = actix_test::TestRequest::post()
            .uri("/security/dtls/revocations")
            .insert_header(("x-forwarded-proto", "https"))
            .insert_header((REVOCATION_SIGNATURE_HEADER, STANDARD.encode(&signature)))
            .set_payload(json.clone())
            .to_request();
        let response = tokio::time::timeout(
            Duration::from_secs(20),
            actix_test::call_service(&app, request),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(response).await;
        assert_eq!(body["sequence"], 2);

        assert_eq!(revocations.snapshot().sequence, 2);
        assert!(revocations
            .snapshot()
            .revoked_serials
            .contains(&vec![1, 175]));
        assert_eq!(
            verifier
                .decode(&fs::read(&state_path).unwrap(), None)
                .unwrap()
                .sequence(),
            2
        );
        let remote = tokio::time::timeout(
            Duration::from_secs(20),
            dht.get_raw_mutable(REVOCATION_DHT_KEY),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(remote.iter().any(|item| *item.seq() == 2));

        let local_address = dht.local_addr().await.unwrap();
        let reader = Arc::new(
            DHTMap::new(
                0,
                &[format!("127.0.0.1:{}", local_address.port())],
                dtls,
                verifier.clone(),
            )
            .unwrap(),
        );
        let locally_served = tokio::time::timeout(
            Duration::from_secs(20),
            reader.get_raw_mutable(REVOCATION_DHT_KEY),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(locally_served.iter().any(|item| *item.seq() == 2));

        let replay = actix_test::TestRequest::post()
            .uri("/security/dtls/revocations")
            .insert_header(("x-forwarded-proto", "https"))
            .insert_header((REVOCATION_SIGNATURE_HEADER, STANDARD.encode(signature)))
            .set_payload(json.clone())
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, replay).await.status(),
            StatusCode::CONFLICT
        );

        let conflicting = actix_test::TestRequest::post()
            .uri("/security/dtls/revocations")
            .insert_header(("x-forwarded-proto", "https"))
            .insert_header((REVOCATION_SIGNATURE_HEADER, STANDARD.encode(sign(&json))))
            .set_payload(json)
            .to_request();
        assert_eq!(
            actix_test::call_service(&app, conflicting).await.status(),
            StatusCode::CONFLICT
        );
        assert!(!dht.is_serving());

        let recovered = tokio::time::timeout(
            Duration::from_secs(20),
            dht.get_raw_mutable(REVOCATION_DHT_KEY),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(recovered.iter().any(|item| *item.seq() == 2));
        assert!(dht.get_value("application").await.is_err());
        drop(registry);
        drop(reader);
        drop(dht);
        drop(storage_dht);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn startup_retries_until_late_peer_activates() {
        let reserved = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = reserved.local_addr().unwrap();
        let (verifier, json, signature) = signed_artifact(Vec::new());
        let initial = verifier.verify(json, signature).unwrap();
        let directory =
            std::env::temp_dir().join(format!("tfs-late-peer-{}", rand::random::<u64>()));
        let make_map = |port, peers: &[String]| {
            let (dtls, revocations) = test_dtls();
            revocations.replace(initial.snapshot.clone());
            let verifier =
                Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations).unwrap());
            verifier.set_canonical(&initial);
            let map = Arc::new(DHTMap::new(port, peers, dtls, verifier.clone()).unwrap());
            (map, verifier)
        };
        let (map, verifier) = make_map(0, &[address.to_string()]);
        let registry = DtlsRevocationRegistry::new(
            verifier,
            map.clone(),
            directory.join("first"),
            initial.clone(),
        );
        let activate = registry.activate(true);
        let late_peer = async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert!(!map.is_serving());
            drop(reserved);
            let (peer, verifier) = make_map(address.port(), &[]);
            let registry = DtlsRevocationRegistry::new(
                verifier,
                peer.clone(),
                directory.join("late"),
                initial.clone(),
            );
            registry.activate(false).await.unwrap();
            (peer, registry)
        };
        let (result, peer) = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::join!(activate, late_peer)
        })
        .await
        .unwrap();
        result.unwrap();
        assert!(map.is_serving());
        drop(peer);
        fs::remove_dir_all(directory).unwrap();
    }
}
