use bytes::Bytes;
use futures::{Stream, StreamExt};
use hmac::{Hmac, Mac};
use jwt::SignWithKey;
use sha2::Sha256;
use tokio::pin;
use crate::common::default_error::DefaultError;
use crate::common::models::{JwtToken, TokenAction};

pub async fn read_stream_to_string(mut stream: impl Stream<Item=Result<Bytes, String>>) -> Result<String, String> {
    pin!(stream);
    let mut buffer: Vec<u8> = Vec::with_capacity(50);
    while let Some(some) = stream.next().await {
        if let Ok(v) = some {
            buffer.extend(v.as_ref())
        } else {
            return Err(format!("error when read file. err = {}", some.err().unwrap()))
        }
    }
    Ok(String::from_utf8(buffer).default_res()?)
}

pub fn generate_token(uid: u64, expires: u64, token_action: TokenAction, secret: &[u8]) -> String {
    let hmac: Hmac<Sha256> = Hmac::new_from_slice(secret).unwrap();

    let token = JwtToken {
        expires,
        uid,
        token_action
    };

    token.sign_with_key(&hmac).unwrap()
}