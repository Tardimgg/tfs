use std::{thread, time::Duration};

use mainline::Dht;

use tracing::Level;
use tracing_subscriber;

mod common;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .init();

    let _dht = Dht::client(common::dtls_config()).expect("failed to start DTLS DHT client");

    thread::sleep(Duration::from_secs(5));
}
