use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Padding,
    sign::{RsaPssSaltlen, Signer},
};
use serde_json::Value;

pub(crate) fn sign_ps256(header: &str, claims: &Value, key: &PKey<Private>) -> String {
    let header = URL_SAFE_NO_PAD.encode(header);
    let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signed = format!("{header}.{claims}");
    let mut signer = Signer::new(MessageDigest::sha256(), key).unwrap();
    signer.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
    signer.set_rsa_mgf1_md(MessageDigest::sha256()).unwrap();
    signer.set_rsa_pss_saltlen(RsaPssSaltlen::DIGEST_LENGTH).unwrap();
    let signature = signer.sign_oneshot_to_vec(signed.as_bytes()).unwrap();
    format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
}
