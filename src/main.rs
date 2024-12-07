
mod heartbeat;
mod controllers;
mod services;
mod common;

use libtorrent_sys::ffi::*;

use std::io::{self, Write};
use std::{thread, time::Duration};
use std::any::Any;
use std::cell::RefCell;
use std::error::Error;
use std::fmt::Display;
use std::fs::File;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::str::FromStr;
use std::thread::sleep;
use std::time::SystemTime;
use actix_web::{App, HttpServer};
use actix_web::web::Data;
use async_trait::async_trait;
use mainline::{Dht, Id};
use mainline::dht::DhtSettings;
use mainline::server::DhtServer;
use crate::services::dht_map::DHTMap;
use crate::services::distributed_map::DistributedMap;
use crate::services::distributed_map::error::{GetError, PutError};
use crate::services::file_storage::local_storage::LocalStorage;
use crate::services::virtual_fs::VirtualFS;

struct Mock {

}

#[async_trait]
impl DistributedMap for Mock {
    async fn put(&self, key: &str, value: &str) -> Result<(), PutError> {
        todo!()
    }

    async fn get(&self, key: &str) -> Result<Vec<String>, GetError> {
        todo!()
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    tokio::spawn(heartbeat::init());

    let virtual_fs = VirtualFS::builder()
        .map(Box::new(Mock {}))
        .storage(Box::new(LocalStorage {}))
        .build();

    // let data = Data::new(Rc::new(virtual_fs));
    let data = Data::new(virtual_fs);

    /*
    let app = App::new()
        // Store `MyData` in application storage.
        .app_data(Data::clone(&data))
        .service(controllers::virtual_fs_controller::config());
        // .route("/index.html", web::get().to(index))
        // .route("/index-alt.html", web::get().to(index_alt));

     //
     */
    HttpServer::new(move || {
        App::new()
            .app_data(Data::clone(&data))
            .service(controllers::virtual_fs_controller::config())
    }).bind(("0.0.0.0", 8080))?
        .run()
        .await.unwrap();

    return Ok(())


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