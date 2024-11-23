mod error;

use std::any::Any;
use std::io::Write;
use flume::{Receiver, Sender};
use mainline::{Bytes, Dht, Id, MutableItem, SigningKey};
use mainline::async_dht::AsyncDht;
use tokio::io::AsyncReadExt;
use typed_builder::TypedBuilder;
use crate::distributed_map::error::{CreateError, GetError, KeyError, PutError};
use futures::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use sha2::digest::Update;

struct DistributedMap {
    dht: AsyncDht
}

impl DistributedMap {

    fn new() -> Result<DistributedMap, CreateError> {
        let dht = Dht::builder().server().build()?;
        let map = DistributedMap {
            dht: dht.as_async()
        };

        return Ok(map);
    }


    async fn put(&self, key: &str, value: &str) -> Result<(), PutError> {
        // Dht::server().unwrap().
        // dbg!()

        let key = self.generate_key(key)?;
        let item = MutableItem::new(key, Bytes::from(value.to_string()), 0, None);
        let res = self.dht.put_mutable(item).await;
        dbg!("id", res.unwrap().bytes);


        return Ok(())
    }

    async fn get(&self, key: &str) -> Result<Option<String>, GetError> {

        // let (tx, rx): (Sender<i32>, Receiver<i32>) = flume::unbounded();
        // let (tx1, rx1) = flume::unbounded();
        //
        // let a = 2;
        // let mut b = rx.into_stream();

        // let mut c = rx1.into_stream();
        // let mut a: RecvStream<MutableItem> = self.dht.get_mutable(&[0u8; 32], None, None)?;



        let path = "/home/tardimgg/file.txt";

        let key = self.generate_key(path)?;

        let mut a = self.dht.get_mutable(key.verifying_key().as_bytes(), None, None)?;

        let t = a.next();
        let t1 = a.map(|v| dbg!(v));
        // c.read_i64();
        // a.read_i32();


        return Ok(None);
    }

    fn generate_key(&self, key: &str) -> Result<SigningKey, KeyError> {
        let mut hasher = Sha256::new();
        hasher.write(key.as_bytes())?;

        let private_key = hasher.finalize();
        Ok(SigningKey::try_from(private_key.as_slice())?)
    }
}

/*
приватный ключ -> путь
публичный ключ -> все что хочет может получить -> можем его лукапть

есть проблема, что приватный ключ всегда 32 байта => хеш от пути, => коллизии => оп одному пути могут хранится совершенно разные данные
можно в данных зранить их настоящий путь

либо структуру вида

getByPublicKey(getPublicKeyBySecret(hash(path))) =
{
    "path": [ip, ip2]
    "path2": [ip, ip2]

}

даже +- секьюрно (похуй), тк это все на серверах, клиенту вернётся только ip по его пути, к которому у него есть доступ

так ок, теперь как хранить права, и инфу о загрузке

хотя, если мы теперь умеем хранить key -> value, то все готов (похуй)

 */