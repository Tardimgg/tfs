use std::{env, fs};

use mainline::{DtlsConfig, RevocationStore};

pub fn dtls_config() -> DtlsConfig {
    DtlsConfig::from_pem(
        &read("DTLS_NODE_CERT_PATH"),
        &read("DTLS_NODE_KEY_PATH"),
        &read("DTLS_ROOT_CA_PATH"),
        RevocationStore::default(),
    )
    .expect("invalid DTLS identity")
}

fn read(variable: &str) -> Vec<u8> {
    let path = env::var(variable).unwrap_or_else(|_| panic!("{} is required", variable));
    fs::read(path).unwrap_or_else(|error| panic!("failed to read {}: {}", variable, error))
}
