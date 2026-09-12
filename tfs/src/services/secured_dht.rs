use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mainline::server::{DhtServer, DhtServerSettings, MutableItemValidator};
use mainline::{Dht, DhtSecurityState, DtlsConfig, MutableItem, RevocationSnapshot};
use tokio_util::sync::CancellationToken;

use crate::services::dht_map::error::{CreateError, DhtGetError, DhtPutError};
use crate::services::dtls_revocation_registry::REVOCATION_DHT_KEY;

mod raw;
pub(crate) use raw::generate_key;
use raw::RawDht;

struct SharedDht {
    raw: RawDht,
    serving: Arc<AtomicBool>,
    revocations: DhtSecurityState,
    security_failure: CancellationToken,
}

impl SharedDht {
    fn check_security(&self) -> Result<(), mainline::Error> {
        if self.revocations.security_failed() {
            Err(mainline::Error::SecurityFailed)
        } else {
            Ok(())
        }
    }

    async fn guarded_operation<T>(
        &self,
        check: impl Fn() -> Result<(), mainline::Error>,
        operation: impl Future<Output = T>,
    ) -> Result<T, mainline::Error> {
        check()?;
        tokio::select! {
            biased;
            _ = self.security_failure.cancelled() => Err(mainline::Error::SecurityFailed),
            result = operation => {
                check()?;
                Ok(result)
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct SecuredDht {
    shared: Arc<SharedDht>,
}

pub(crate) struct RevocationAdmin {
    shared: Arc<SharedDht>,
}

impl SecuredDht {
    pub(crate) fn new(
        port: u16,
        other: &[String],
        dtls: DtlsConfig,
        mutable_item_validator: Arc<dyn MutableItemValidator>,
    ) -> Result<(Self, RevocationAdmin), CreateError> {
        let serving = Arc::new(AtomicBool::new(false));
        let revocations = dtls.revocations().clone();
        let server = DhtServer::new(&DhtServerSettings {
            mutable_item_validator: Some(mutable_item_validator),
            request_gate: Some(serving.clone()),
            ..DhtServerSettings::default()
        });
        let dht = Dht::builder()
            .custom_server(Box::new(server))
            .bootstrap(other)
            .port(port)
            .dtls(dtls)
            .build()?;
        let shared = Arc::new(SharedDht {
            raw: RawDht::new(dht.as_async()),
            serving,
            revocations,
            security_failure: CancellationToken::new(),
        });
        Ok((Self { shared: shared.clone() }, RevocationAdmin { shared }))
    }

    pub(crate) fn check_security(&self) -> Result<(), mainline::Error> {
        self.shared.check_security()
    }

    pub(crate) async fn application_operation<T>(
        &self,
        operation: impl Future<Output = T>,
    ) -> Result<T, mainline::Error> {
        self.shared
            .guarded_operation(|| self.check_security(), operation)
            .await
    }

    pub(crate) async fn get_raw_mutable(&self, key: &str) -> Result<Vec<MutableItem>, DhtGetError> {
        self.application_operation(self.shared.raw.get(key)).await?
    }

    pub(crate) async fn put_raw_mutable(
        &self,
        key: &str,
        value: Vec<u8>,
        sequence: i64,
        cas: Option<i64>,
    ) -> Result<(), DhtPutError> {
        self.application_operation(self.shared.raw.put(key, value, sequence, cas)).await?
    }
}

impl RevocationAdmin {
    pub(crate) fn check_security(&self) -> Result<(), mainline::Error> {
        self.shared.check_security()
    }

    pub(crate) fn check_network(&self) -> Result<(), mainline::Error> {
        self.check_security()?;
        if !self.shared.serving.load(Ordering::Acquire) {
            return Err(mainline::Error::Static("revocation network operations require DHT serving to be open"));
        }
        Ok(())
    }

    pub(crate) async fn wait_for_security_failure(&self) {
        self.shared.security_failure.cancelled().await;
    }

    pub(crate) async fn fetch(&self) -> Result<Vec<MutableItem>, DhtGetError> {
        self.shared
            .guarded_operation(|| self.check_network(), self.shared.raw.get(REVOCATION_DHT_KEY))
            .await?
    }

    pub(crate) async fn publish(&self, value: Vec<u8>, sequence: i64) -> Result<(), DhtPutError> {
        self.shared
            .guarded_operation(|| self.check_network(), self.shared.raw.put(REVOCATION_DHT_KEY, value, sequence, None))
            .await?
    }

    pub(crate) async fn install_protected(
        &self,
        value: Vec<u8>,
        sequence: i64,
    ) -> Result<bool, DhtPutError> {
        self.shared.raw.install_protected(REVOCATION_DHT_KEY, value, sequence).await
    }

    pub(crate) async fn protected(&self) -> Result<Option<MutableItem>, DhtGetError> {
        self.shared.raw.protected(REVOCATION_DHT_KEY).await
    }

    pub(crate) async fn replace_dtls_revocations(
        &self,
        snapshot: RevocationSnapshot,
    ) -> Result<bool, mainline::Error> {
        self.shared.raw.replace_dtls_revocations(snapshot).await
    }

    pub(crate) async fn start_serving_if_protected_matches(
        &self,
        value: Vec<u8>,
        sequence: i64,
    ) -> Result<bool, DhtPutError> {
        self.check_security()?;
        let started = self.shared.raw
            .start_serving_if_protected_matches(REVOCATION_DHT_KEY, value, sequence).await?;
        if let Err(error) = self.check_security() {
            self.stop_serving();
            return Err(error.into());
        }
        Ok(started)
    }

    fn stop_serving(&self) {
        self.shared.serving.store(false, Ordering::Release);
    }

    pub(crate) fn fail_security(&self) {
        self.shared.revocations.fail_security();
        self.shared.security_failure.cancel();
        self.stop_serving();
    }

    #[cfg(test)]
    pub(crate) fn is_serving(&self) -> bool {
        self.shared.serving.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) async fn local_addr(&self) -> Result<std::net::SocketAddr, mainline::Error> {
        self.shared.raw.local_addr().await
    }
}
