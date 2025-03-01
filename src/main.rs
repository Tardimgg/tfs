
mod heartbeat;
mod controllers;
mod services;
mod common;
mod config;

use libtorrent_sys::ffi::*;

use std::io::{self, Write};
use std::{env, thread, time::Duration};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::Display;
use std::fs::File;
use std::marker::PhantomData;
use std::net::{IpAddr, SocketAddr};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use std::thread::sleep;
use std::time::SystemTime;
use actix::fut::try_future::MapErr;
use actix_web::{App, HttpServer, web};
use actix_web::rt::signal;
use actix_web::web::Data;
use async_trait::async_trait;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use crate::config::global_config::GlobalConfig;
use crate::heartbeat::Heartbeat;
use crate::services::dht_map::dht_map::DHTMap;
use crate::services::dht_map::model::DhtNodeId;
use crate::services::file_storage::local_storage::local_storage::LocalStorage;
use crate::services::virtual_fs::VirtualFS;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), AppError> {
    // let page = DhtPage::from("string".to_string());
    // let data: TfsData = TfsData::from(ListOfIP::from(page));
    //
    //
    // vfs.getData(vfs.getPag(dht.getIps("string".to_string())))
    //
    // vfs.getData(vfs.getPag2(dht2.getIps("string".to_string())))
    let args: Vec<String> = env::args().collect();
    let port = args.get(1).map(|v| v.parse::<u16>().unwrap_or(8080)).unwrap_or(8080);

    let other_ports = args.iter().skip(2).map(|v| v.parse::<u16>().unwrap()).collect::<Vec<u16>>();
    let other_ips = other_ports.into_iter().map(|v| format!("{}:{}", "127.0.0.1", v)).collect::<Vec<String>>();

    println!("used port: {}", port);
    println!("other_ips: {:?}", other_ips);
    let dht = DHTMap::new(port, &other_ips.into_iter().map(|v| v.to_string()).collect::<Vec<String>>()).unwrap();

    let dht = Arc::new(dht);


    let dht_node_id = DhtNodeId::builder()
        .ip(IpAddr::from([127, 0, 0, 1]))
        .port(port)
        .build();

    let config = Arc::new(GlobalConfig::new(dht.clone()));
    let storage = Arc::new(LocalStorage{ base_path: "root".to_string() });

    let (tx, rx) = mpsc::channel(32);
    let virtual_fs = VirtualFS::builder()
        .dht_map(dht.clone())
        .id(dht_node_id)
        .storage(storage.clone())
        .config(config.clone())
        .state_updater(tx)
        .build();
    let virtual_fs = Arc::new(virtual_fs);

    let cancellation_token = tokio_util::sync::CancellationToken::new();


    let cloned_fs = virtual_fs.clone();
    let cloned_config = config.clone();
    let cloned_storage = storage.clone();
    let cloned_cancellation_token = cancellation_token.clone();
    tokio::task::spawn_blocking(|| {
        Handle::current().block_on(async move {
            let heartbeat = Heartbeat::builder()
                .fs(cloned_fs)
                .config(cloned_config)
                .storage(cloned_storage)
                .cancellation_token(cloned_cancellation_token)
                .build();

            heartbeat.init(rx).await;
        });
    });

    tokio::spawn(async move {
        match signal::ctrl_c().await {
            Ok(()) => {
                cancellation_token.cancel();
            },
            Err(err) => {
                cancellation_token.cancel();
                eprintln!("Unable to listen for shutdown signal: {}", err);
            },
        }
    });

    HttpServer::new(move || {
        App::new()
            .app_data(dht.clone())
            .app_data(config.clone())
            .app_data(virtual_fs.clone())
            // .app_data(web::PayloadConfig::new(1 * 1024 * 1024 * 1024))
            .service(controllers::virtual_fs_controller::virtual_fs_controller::config())
    }).bind(("0.0.0.0", port))?
        .run()
        .await?;
    
    Ok(())


    // server().await;
    // client().await;
}

#[derive(Debug)]
struct AppError {
    err: String
}

impl From<String> for AppError {
    fn from(value: String) -> Self {
        AppError {
            err: value
        }
    }
}

impl From<io::Error> for AppError {
    fn from(value: io::Error) -> Self {
        value.to_string().into()
    }
}


// #[actix_web::main]
// async fn main() -> std::io::Result<()> {
//     HttpServer::new(|| {
//         App::new().service(greet)
    // })
    //     .bind(("127.0.0.1", 8080))?
    //     .run()
    //     .await
// }

async fn server() {
    // let map = DistributedMap::new(8082, &vec!["127.0.0.1:8081".to_string(), "127.0.0.1:8080".to_string()]).unwrap();
    let map = DHTMap::new(8080, &vec!["127.0.0.1:8081".to_string(), "127.0.0.1:8082".to_string()]).unwrap();

    sleep(Duration::from_secs(10 * 60));


    // map.put().unwrap()
}

async fn client() {
/*
    let map = DHTMap::new(8081, &vec!["127.0.0.1:8082".to_string(), "127.0.0.1:8081".to_string()]).unwrap();
    let r = map.get("/home/test123").await.unwrap();
    dbg!(r);

    map.put("/home/test123", "value123").await.unwrap();

    let r = map.get("/home/test123").await.unwrap();
    dbg!(r);

    // tokio::time::sleep(Duration::from_secs(10)).await;
    // let r = map.get("/home/test123").await.unwrap();
    // dbg!(r);

    map.put("/home/test123", "new_value").await.unwrap();

    let r = map.get("/home/test123").await.unwrap();
    dbg!(r);

    tokio::time::sleep(Duration::from_secs(10)).await;
    let r = map.get("/home/test123").await.unwrap();
    dbg!(r);



 */

    // let text = "4238af8aff56cf6e0007d9d2003bf23d33eea7c3";

    /*

    let thread = thread::spawn(|| {
        let dht = Dht::new(DhtSettings {
            bootstrap: Some(vec!["127.0.0.1:8081".to_string()]),
            // bootstrap: None,
            port: Some(8090),
            // server: None,
            server: Some(Box::<DhtServer>::default()),
            request_timeout: None,
        }).unwrap();
        let info_hash = Id::from_str(text).unwrap();

        // let res = dht.announce_peer(Id::from_str(text).unwrap(), None);


        let start = SystemTime::now();;
        for _ in 0..1500 {

            for peer in dht.get_peers(info_hash).unwrap() {
                println!("{:?}", peer);
                sleep(Duration::from_secs(1));
                println!("{:?}", (start.elapsed().unwrap()));
            }
        }
        // println!("{:?}", res);
    });

     */

    /*
    let s1 = thread::spawn(|| {
        let mut dht = Dht::new(DhtSettings {
            bootstrap: Some(vec!["127.0.0.1:8081".to_string()]),
            // bootstrap: None,
            port: Some(8085),
            server: Some(Box::<DhtServer>::default()),
            request_timeout: None,
        }).unwrap();
        let info_hash = Id::from_str(text).unwrap();

        dht.announce_peer(info_hash, None);
        sleep(Duration::from_secs(10));
        dht.shutdown().expect("TODO: panic message");
        panic!("shutdown");
    });


    let s2 = thread::spawn(|| {
        let mut dht = Dht::new(DhtSettings {
            bootstrap: Some(vec!["127.0.0.1:8082".to_string()]),
            // bootstrap: None,
            port: Some(8081),
            server: Some(Box::<DhtServer>::default()),
            request_timeout: None,
        }).unwrap();

        let info_hash = Id::from_str(text).unwrap();
        sleep(Duration::from_secs(5));
        println!("{:?}", dht.get_peers(info_hash).unwrap().into_iter().collect::<Vec<Vec<SocketAddr>>>());

        // let start = SystemTime::now();;
        // for _ in 0..1500 {
        //
        //     for peer in dht.get_peers(info_hash).unwrap() {
        //         println!("2: {:?}", peer);
        //         sleep(Duration::from_secs(1));
        //         println!("2: {:?}", (start.elapsed().unwrap()));
        //     }
        // }

        // dht.announce_peer(info_hash, None);
        sleep(Duration::from_secs(1000000));
        // dht.
    });

     */


    // let start = SystemTime::now();;
    // for _ in 0..1500 {
    //     println!("{:?}", dht.get_peers(info_hash));
    //     sleep(Duration::from_secs(1));
    //     println!("{:?}", (start.elapsed().unwrap()));
    // }
    // println!("{:?}", res);
    // sleep(Duration::from_secs(10));
    // println!("join");
    // dht.shutdown();
    // thread.join().unwrap();
    // s1.join();
    // s2.join();

    /*

    let r = vec![1, 2, 3];
    r.split_at_mut(5);
    let uri = std::env::args().nth(1).expect("no pattern given");
    let mut session = lt_create_session();
    let mut torrent_param = lt_parse_magnet_uri(&uri, ".");
    let hdl = lt_session_add_torrent(session.pin_mut(), torrent_param.pin_mut());
    // session.pin_mut().type_id()

    loop {
        if lt_torrent_has_metadata(&hdl) {
            lt_session_pause(session.pin_mut());
            let torrent_name = lt_torrent_get_name(&hdl);
            println!("\ncreate file: {}.torrent", torrent_name);
            io::stdout().flush().unwrap();

            let bin = lt_torrent_bencode(&hdl);
            let mut ofile = File::create(format!("{}.torrent", torrent_name)).expect("unable to create file");
            ofile.write_all(&bin).expect("unable to write");

            lt_session_remove_torrent(session.pin_mut(), &hdl);
            break;
        }

        print!(".");
        io::stdout().flush().unwrap();
        thread::sleep(Duration::from_millis(1000));
    }

     */
}