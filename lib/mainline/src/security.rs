use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::io::BufReader;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use dtls::cipher_suite::CipherSuiteId;
use dtls::config::{ClientAuthType, Config, ExtendedMasterSecretType};
use dtls::crypto::{Certificate, CryptoPrivateKey, CryptoPrivateKeyKind};
use dtls::signature_hash_algorithm::SignatureScheme;
use rcgen::KeyPair;
use rustls::pki_types::CertificateDer;
use rustls::RootCertStore;
use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::{Error, Result};

const DEFAULT_DTLS_SERVER_NAME: &str = "dht.tfs.internal";
pub(crate) const DEFAULT_DTLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CertificateId {
    pub issuer_spki_sha256: [u8; 32],
    pub serial: Vec<u8>,
    pub spki_sha256: [u8; 32],
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RevocationSnapshot {
    pub sequence: i64,
    pub issuer_spki_sha256: [u8; 32],
    pub revoked_serials: HashSet<Vec<u8>>,
}

#[derive(Debug, Default)]
struct RevocationState {
    security_failed: bool,
    expected_issuer_spki_sha256: Option<[u8; 32]>,
    snapshot: RevocationSnapshot,
}

#[derive(Clone, Debug, Default)]
pub struct RevocationStore(Arc<RwLock<RevocationState>>);

impl RevocationStore {
    /// Latch a fatal trust failure. Only a process restart may clear it.
    pub fn fail_security(&self) {
        self.0
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .security_failed = true;
    }

    pub fn security_failed(&self) -> bool {
        self.0
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .security_failed
    }

    /// Serialize result delivery with the fatal latch. The callback must not block.
    pub(crate) fn with_trust<T>(&self, recovery: bool, deliver: impl FnOnce() -> T) -> Option<T> {
        let state = self.0.read().unwrap_or_else(|p| p.into_inner());
        if state.security_failed && !recovery {
            None
        } else {
            Some(deliver())
        }
    }

    pub fn snapshot(&self) -> RevocationSnapshot {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot
            .clone()
    }

    pub fn replace(&self, snapshot: RevocationSnapshot) -> bool {
        let mut current = self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current
            .expected_issuer_spki_sha256
            .is_some_and(|issuer| issuer != snapshot.issuer_spki_sha256)
            || snapshot.sequence <= current.snapshot.sequence
        {
            return false;
        }
        current.snapshot = snapshot;
        true
    }

    pub fn is_revoked(&self, certificate: &CertificateId) -> bool {
        let current = self
            .0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        current.snapshot.issuer_spki_sha256 == certificate.issuer_spki_sha256
            && current
                .snapshot
                .revoked_serials
                .contains(&certificate.serial)
    }

    fn bind_issuer(&self, issuer_spki_sha256: [u8; 32]) -> bool {
        let mut current = self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match current.expected_issuer_spki_sha256 {
            Some(expected) => expected == issuer_spki_sha256,
            None => {
                if current.snapshot.sequence > 0
                    && current.snapshot.issuer_spki_sha256 != issuer_spki_sha256
                {
                    return false;
                }
                current.expected_issuer_spki_sha256 = Some(issuer_spki_sha256);
                true
            }
        }
    }
}

#[derive(Clone)]
pub struct DtlsConfig {
    certificate: Certificate,
    roots: RootCertStore,
    issuer_spki_sha256: [u8; 32],
    issuer_name: Vec<u8>,
    root_certificate: CertificateDer<'static>,
    server_name: String,
    handshake_timeout: Duration,
    revocations: RevocationStore,
}

impl Debug for DtlsConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DtlsConfig")
            .field("certificate_chain_len", &self.certificate.certificate.len())
            .field("issuer_spki_sha256", &self.issuer_spki_sha256)
            .field("server_name", &self.server_name)
            .field("handshake_timeout", &self.handshake_timeout)
            .field("revocations", &self.revocations)
            .finish_non_exhaustive()
    }
}

impl DtlsConfig {
    pub fn from_pem(
        certificate_chain_pem: &[u8],
        private_key_pem: &[u8],
        root_ca_pem: &[u8],
        revocations: RevocationStore,
    ) -> Result<Self> {
        let certificate_chain = parse_certificates(certificate_chain_pem)?;
        if certificate_chain.len() != 1 {
            return Err(Error::InvalidDtlsIdentity(
                "DTLS identity must contain exactly one leaf certificate".to_string(),
            ));
        }

        let private_key_pem = std::str::from_utf8(private_key_pem)
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        let key_pair = KeyPair::from_pem(private_key_pem)
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        let private_key = CryptoPrivateKey::from_key_pair(&key_pair)
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        if !matches!(private_key.kind, CryptoPrivateKeyKind::Ecdsa256(_)) {
            return Err(Error::InvalidDtlsIdentity(
                "DTLS private key must use P-256".to_string(),
            ));
        }

        let root_certificates = parse_certificates(root_ca_pem)?;
        if root_certificates.len() != 1 {
            return Err(Error::InvalidDtlsIdentity(
                "DTLS requires exactly one root CA certificate".to_string(),
            ));
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(root_certificates[0].clone())
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        let issuer_spki_sha256 = certificate_spki_sha256(root_certificates[0].as_ref())?;
        if !revocations.bind_issuer(issuer_spki_sha256) {
            return Err(Error::InvalidDtlsIdentity(
                "revocation store is bound to a different root CA".to_string(),
            ));
        }
        let (_, leaf_certificate) = X509Certificate::from_der(certificate_chain[0].as_ref())
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        let (_, root_certificate) = X509Certificate::from_der(root_certificates[0].as_ref())
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        if leaf_certificate.public_key().raw != key_pair.public_key_der() {
            return Err(Error::InvalidDtlsIdentity(
                "DTLS certificate does not match its private key".to_string(),
            ));
        }
        if leaf_certificate.issuer().as_raw() != root_certificate.subject().as_raw() {
            return Err(Error::InvalidDtlsIdentity(
                "DTLS certificate must be issued directly by the configured root CA".to_string(),
            ));
        }
        let issuer_name = root_certificate.subject().as_raw().to_vec();

        Ok(Self {
            certificate: Certificate {
                certificate: certificate_chain,
                private_key,
            },
            roots,
            issuer_spki_sha256,
            issuer_name,
            root_certificate: root_certificates[0].clone(),
            server_name: DEFAULT_DTLS_SERVER_NAME.to_string(),
            handshake_timeout: DEFAULT_DTLS_HANDSHAKE_TIMEOUT,
            revocations,
        })
    }

    pub fn with_server_name(mut self, server_name: impl Into<String>) -> Self {
        self.server_name = server_name.into();
        self
    }

    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    pub fn revocations(&self) -> &RevocationStore {
        &self.revocations
    }

    pub fn issuer_spki_sha256(&self) -> [u8; 32] {
        self.issuer_spki_sha256
    }

    pub(crate) fn handshake_timeout(&self) -> Duration {
        self.handshake_timeout
    }

    pub(crate) fn certificate_id(&self, certificate_der: &[u8]) -> Result<CertificateId> {
        let (_, certificate) = X509Certificate::from_der(certificate_der)
            .map_err(|error| Error::InvalidDtlsPeerCertificate(error.to_string()))?;
        if certificate.issuer().as_raw() != self.issuer_name {
            return Err(Error::InvalidDtlsPeerCertificate(
                "certificate was not issued directly by the configured root CA".to_string(),
            ));
        }
        let (_, root) = X509Certificate::from_der(&self.root_certificate)
            .map_err(|error| Error::InvalidDtlsPeerCertificate(error.to_string()))?;
        let profile_error = || {
            Error::InvalidDtlsPeerCertificate(
                "peer must be a directly root-signed P-256 leaf with the configured DNS SAN and both clientAuth and serverAuth EKUs".to_string(),
            )
        };
        let algorithm = &certificate.public_key().algorithm;
        if algorithm.algorithm.to_id_string() != "1.2.840.10045.2.1"
            || algorithm
                .parameters
                .as_ref()
                .and_then(|parameters| parameters.as_oid().ok())
                .is_none_or(|curve| curve.to_id_string() != "1.2.840.10045.3.1.7")
            || certificate.is_ca()
            || !certificate
                .extended_key_usage()
                .map_err(|_| profile_error())?
                .is_some_and(|eku| eku.value.client_auth && eku.value.server_auth)
            || !certificate
                .subject_alternative_name()
                .map_err(|_| profile_error())?
                .is_some_and(|san| san.value.general_names.iter().any(|name| {
                    matches!(name, x509_parser::extensions::GeneralName::DNSName(dns) if dns.eq_ignore_ascii_case(&self.server_name))
                }))
        {
            return Err(profile_error());
        }
        certificate
            .verify_signature(Some(root.public_key()))
            .map_err(|_| profile_error())?;
        Ok(CertificateId {
            issuer_spki_sha256: self.issuer_spki_sha256,
            serial: certificate.raw_serial().to_vec(),
            spki_sha256: Sha256::digest(certificate.public_key().raw).into(),
        })
    }

    pub fn local_certificate_id(&self) -> Result<CertificateId> {
        let certificate = self.certificate.certificate.first().ok_or_else(|| {
            Error::InvalidDtlsIdentity("certificate chain is empty".to_string())
        })?;
        self.certificate_id(certificate)
    }

    pub(crate) fn connection_config(&self) -> Config {
        let revocations = self.revocations.clone();
        let identity = self.clone();

        Config {
            certificates: vec![self.certificate.clone()],
            cipher_suites: vec![CipherSuiteId::Tls_Ecdhe_Ecdsa_With_Aes_128_Gcm_Sha256],
            signature_schemes: vec![SignatureScheme::EcdsaWithP256AndSha256],
            client_auth: ClientAuthType::RequireAndVerifyClientCert,
            extended_master_secret: ExtendedMasterSecretType::Require,
            roots_cas: self.roots.clone(),
            client_cas: self.roots.clone(),
            server_name: self.server_name.clone(),
            mtu: 1200,
            replay_protection_window: 64,
            verify_peer_certificate: Some(Arc::new(move |raw_certificates, _| {
                let raw_certificate = raw_certificates
                    .first()
                    .ok_or(dtls::Error::ErrNoCertificates)?;
                let certificate_id = identity
                    .certificate_id(raw_certificate)
                    .map_err(|error| dtls::Error::Other(error.to_string()))?;
                if revocations.is_revoked(&certificate_id) {
                    return Err(dtls::Error::Other(
                        "peer certificate is revoked".to_string(),
                    ));
                }
                Ok(())
            })),
            ..Default::default()
        }
    }
}

fn parse_certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    rustls_pemfile::certs(&mut BufReader::new(pem))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))
}

fn certificate_spki_sha256(certificate_der: &[u8]) -> Result<[u8; 32]> {
    let (_, certificate) = X509Certificate::from_der(certificate_der)
        .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
    Ok(Sha256::digest(certificate.public_key().raw).into())
}

pub(crate) fn test_dtls_configs(count: usize) -> Result<Vec<DtlsConfig>> {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose,
        SerialNumber, PKCS_ECDSA_P256_SHA256,
    };

    let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new())
        .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_certificate = ca_params
        .self_signed(&ca_key)
        .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
    let ca_der = ca_certificate.der().clone();
    let issuer_spki_sha256 = certificate_spki_sha256(ca_der.as_ref())?;
    let (_, parsed_ca) = X509Certificate::from_der(ca_der.as_ref())
        .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
    let issuer_name = parsed_ca.subject().as_raw().to_vec();

    let mut configs = Vec::with_capacity(count);
    for index in 0..count {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        let mut params = CertificateParams::new(vec![DEFAULT_DTLS_SERVER_NAME.to_string()])
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        params.serial_number = Some(SerialNumber::from((index + 1) as u64));
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];
        let certificate = params
            .signed_by(&key_pair, &ca_certificate, &ca_key)
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        let private_key = CryptoPrivateKey::from_key_pair(&key_pair)
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;
        let mut roots = RootCertStore::empty();
        roots
            .add(ca_der.clone())
            .map_err(|error| Error::InvalidDtlsIdentity(error.to_string()))?;

        let revocations = RevocationStore::default();
        let _ = revocations.bind_issuer(issuer_spki_sha256);
        configs.push(DtlsConfig {
            certificate: Certificate {
                certificate: vec![certificate.der().clone()],
                private_key,
            },
            roots,
            issuer_spki_sha256,
            issuer_name: issuer_name.clone(),
            root_certificate: ca_der.clone(),
            server_name: DEFAULT_DTLS_SERVER_NAME.to_string(),
            handshake_timeout: DEFAULT_DTLS_HANDSHAKE_TIMEOUT,
            revocations,
        });
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose,
        PKCS_ECDSA_P256_SHA256,
    };

    use super::*;

    fn pem_identity() -> (String, String, String) {
        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca = ca_params.self_signed(&ca_key).unwrap();

        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_params =
            CertificateParams::new(vec![DEFAULT_DTLS_SERVER_NAME.to_string()]).unwrap();
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];
        let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();
        (leaf.pem(), leaf_key.serialize_pem(), ca.pem())
    }

    #[test]
    fn loads_matching_p256_identity() {
        let (certificate, key, root) = pem_identity();
        let config = DtlsConfig::from_pem(
            certificate.as_bytes(),
            key.as_bytes(),
            root.as_bytes(),
            RevocationStore::default(),
        )
        .unwrap();

        assert_eq!(
            config.local_certificate_id().unwrap().issuer_spki_sha256,
            config.issuer_spki_sha256()
        );
    }

    #[test]
    fn rejects_certificate_key_mismatch() {
        let (certificate, _, root) = pem_identity();
        let mismatched_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .unwrap()
            .serialize_pem();
        assert!(matches!(
            DtlsConfig::from_pem(
                certificate.as_bytes(),
                mismatched_key.as_bytes(),
                root.as_bytes(),
                RevocationStore::default(),
            ),
            Err(Error::InvalidDtlsIdentity(_))
        ));
    }

    #[test]
    fn peer_profile_requires_both_ekus_san_p256_and_direct_signature() {
        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let mut config = test_dtls_configs(1).unwrap().remove(0);
        config.root_certificate = ca.der().clone();
        let (_, parsed) = X509Certificate::from_der(ca.der()).unwrap();
        config.issuer_name = parsed.subject().as_raw().to_vec();
        for case in 0..7 {
            let algorithm = if case == 4 {
                &rcgen::PKCS_ECDSA_P384_SHA384
            } else {
                &PKCS_ECDSA_P256_SHA256
            };
            let key = KeyPair::generate_for(algorithm).unwrap();
            let mut params = CertificateParams::new(if case == 3 {
                vec![]
            } else {
                vec![if case == 2 {
                    "wrong.internal"
                } else {
                    DEFAULT_DTLS_SERVER_NAME
                }
                .to_string()]
            })
            .unwrap();
            params.extended_key_usages = match case {
                0 => vec![ExtendedKeyUsagePurpose::ClientAuth],
                1 => vec![ExtendedKeyUsagePurpose::ServerAuth],
                _ => vec![
                    ExtendedKeyUsagePurpose::ClientAuth,
                    ExtendedKeyUsagePurpose::ServerAuth,
                ],
            };
            let impostor_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let leaf = params
                .signed_by(&key, &ca, if case == 5 { &impostor_key } else { &ca_key })
                .unwrap();
            let result = config.certificate_id(leaf.der());
            assert_eq!(result.is_ok(), case == 6, "case {case}: {result:?}");
            let callback = config.connection_config().verify_peer_certificate.unwrap();
            assert_eq!(callback(&[leaf.der().to_vec()], &[]).is_ok(), case == 6);
        }
    }

    #[test]
    fn fatal_security_failure_is_latched_across_snapshot_updates() {
        let store = RevocationStore::default();
        assert!(!store.security_failed());
        store.fail_security();
        assert!(store.replace(RevocationSnapshot {
            sequence: 1,
            ..Default::default()
        }));
        assert!(store.clone().security_failed());
    }

    #[test]
    fn revocation_snapshots_only_advance() {
        let store = RevocationStore::default();
        assert!(store.bind_issuer([1; 32]));
        let certificate = CertificateId {
            issuer_spki_sha256: [1; 32],
            serial: vec![2],
            spki_sha256: [3; 32],
        };
        assert!(store.replace(RevocationSnapshot {
            sequence: 1,
            issuer_spki_sha256: certificate.issuer_spki_sha256,
            revoked_serials: HashSet::from([certificate.serial.clone()]),
        }));
        assert!(store.is_revoked(&certificate));
        assert!(!store.replace(RevocationSnapshot {
            sequence: 1,
            ..RevocationSnapshot::default()
        }));
        assert!(!store.replace(RevocationSnapshot {
            sequence: 2,
            issuer_spki_sha256: [9; 32],
            revoked_serials: HashSet::new(),
        }));
        assert!(store.is_revoked(&certificate));
    }
}
