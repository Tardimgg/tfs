use super::*;

async fn activate_peer(admin: &RevocationAdmin, initial: &VerifiedRevocationArtifact) {
    let encoded = initial.encode().unwrap();
    let sequence = initial.sequence();
    assert!(admin
        .install_protected(encoded.clone(), sequence)
        .await
        .unwrap());
    assert!(admin
        .start_serving_if_protected_matches(encoded, sequence)
        .await
        .unwrap());
}

pub(crate) async fn open_loopback_map() -> (Arc<DHTMap>, RevocationAdmin, &'static [u8]) {
    let initial = test_artifact(1, Vec::new());
    let (_, storage) = open_peer(&[], &initial).await;
    let address = storage.local_addr().await.unwrap();
    let (client, _) = open_peer(&[format!("127.0.0.1:{}", address.port())], &initial).await;
    (Arc::new(DHTMap::new(client)), storage, &ca().pem)
}

pub(super) async fn open_peer(
    bootstrap: &[String],
    initial: &VerifiedRevocationArtifact,
) -> (SecuredDht, RevocationAdmin) {
    let (secured, admin, _) = test_peer(bootstrap, Some(initial));
    activate_peer(&admin, initial).await;
    (secured, admin)
}

pub(super) fn test_peer(
    bootstrap: &[String],
    initial: Option<&VerifiedRevocationArtifact>,
) -> (SecuredDht, RevocationAdmin, Arc<RevocationVerifier>) {
    let (dtls, revocations) = test_dtls();
    let verifier = Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations).unwrap());
    if let Some(initial) = initial {
        assert!(verifier.revocations.replace(initial.snapshot.clone()));
        verifier.set_canonical(initial);
    }
    let (secured, admin) = SecuredDht::new(0, bootstrap, dtls, verifier.clone()).unwrap();
    (secured, admin, verifier)
}
