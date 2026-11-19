use super::*;
use crate::services::dht_map::codec::{DhtMapValue, DhtValue};
use crate::services::dht_map::model::PrevSeq;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

struct OversizedValue {
    attempted: Notify,
    attempts: AtomicUsize,
}

impl DhtMapValue for OversizedValue {
    const TYPE_NAME: &'static str = "String";

    fn to_dht_value(&self) -> DhtValue {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        self.attempted.notify_one();
        DhtValue::Text("value".repeat(64 * 1024))
    }

    fn from_dht_value(_: DhtValue) -> Result<Self, DhtCodecError> {
        panic!("write-only test value")
    }
}

#[tokio::test]
async fn security_failure_cancels_map_puts_during_retry_backoff() {
    for explicit_sequence in [false, true] {
        let (dtls, revocations) = test_dtls();
        let verifier = Arc::new(RevocationVerifier::new(&ca().pem, ca().issuer, revocations).unwrap());
        let (secured, admin) = SecuredDht::new(0, &[], dtls, verifier).unwrap();
        let map = DHTMap::new(secured);
        let value = OversizedValue {
            attempted: Notify::new(),
            attempts: AtomicUsize::new(0),
        };
        let operation = async {
            if explicit_sequence {
                map.put_with_seq("application", &value, PrevSeq::Seq(None)).await
            } else {
                map.put("application", &value).await
            }
        };
        tokio::pin!(operation);
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                result = &mut operation => panic!("put completed before backoff: {result:?}"),
                _ = value.attempted.notified() => {}
            }
        }).await.unwrap();

        admin.fail_security();
        let result = tokio::time::timeout(Duration::from_millis(100), &mut operation).await.unwrap();

        assert!(matches!(result, Err(DhtPutError::Mainline(mainline::Error::SecurityFailed))));
        assert_eq!(value.attempts.load(Ordering::Relaxed), 1);
    }
}
