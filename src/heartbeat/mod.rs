use std::time::Duration;

// static dht: DistributedMap = 0;

pub async fn init() {
    init_db().await;

    let mut interval = tokio::time::interval(Duration::from_millis(10));

    loop {
        interval.tick().await;
        scan_files().await;
    }
}

async fn init_db() {

}
async fn scan_files() {

}