use std::time::Instant;

use mainline::{async_dht::AsyncDht, Bytes, Dht};

use clap::Parser;

use tracing::Level;
use tracing_subscriber;

mod common;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Value to store on the DHT
    value: String,
}

fn main() {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let cli = Cli::parse();

    let dht = Dht::client(common::dtls_config())
        .expect("failed to start DTLS DHT client")
        .as_async();
    let value = Bytes::from(cli.value.to_owned());

    println!("\nStoring immutable data: {} ...\n", cli.value);
    futures::executor::block_on(async {
        println!("\n=== COLD QUERY ===");
        put_immutable(&dht, &value).await;

        println!("\n=== SUBSEQUENT QUERY ===");
        put_immutable(&dht, &value).await;
    });
}

async fn put_immutable(dht: &AsyncDht, value: &Bytes) {
    let start = Instant::now();

    let info_hash = dht
        .put_immutable(value.to_owned())
        .await
        .expect("put immutable failed");

    println!(
        "Stored immutable data as {:?} in {:?} milliseconds",
        info_hash,
        start.elapsed().as_millis()
    );
}
