use std::{str::FromStr, time::Instant};
use std::thread::sleep;
use std::time::Duration;

use mainline::{Dht, Id};

use clap::Parser;
use rand::random;

use tracing::Level;
use tracing_subscriber;
use mainline::dht::DhtSettings;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// info_hash to lookup peers for
    infohash: String,
}

fn main() {
    tracing_subscriber::fmt()
        // Switch to DEBUG to see incoming values and the IP of the responding nodes
        .with_max_level(Level::INFO)
        .init();


    let info_hash = Id::from_str("4238af8aff56cf6e0007d9d2003bf23d33eea7c3").unwrap();

    let dht = Dht::new(DhtSettings {
        bootstrap: Some(vec!["127.0.0.1:8080".to_string()]),
        server: None,
        port: Some(8000),
        request_timeout: None,
    }).unwrap();


    println!("Looking up peers for info_hash: {} ...", info_hash);
    println!("\n=== COLD QUERY ===");

    for _ in 0..1000 {
        get_peers(&dht, &info_hash);
        sleep(Duration::from_secs(1));
    }

    println!("\n=== SUBSEQUENT QUERY ===");
    println!("Looking up peers for info_hash: {} ...", info_hash);
    get_peers(&dht, &info_hash);
}

fn get_peers(dht: &Dht, info_hash: &Id) {
    let start = Instant::now();
    let mut first = false;

    let mut count = 0;

    for peer in dht.get_peers(*info_hash).unwrap() {
        if !first {
            first = true;
            println!(
                "Got first result in {:?} milliseconds:",
                start.elapsed().as_millis()
            );

            println!("peer {:?}", peer,);
        }

        count += 1;
    }

    println!(
        "\nQuery exhausted in {:?} milliseconds, got {:?} peers.",
        start.elapsed().as_millis(),
        count
    );
}
