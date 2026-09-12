use actix_web::error::{ErrorServiceUnavailable, ErrorUnauthorized};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use openssl::error::ErrorStack;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Public};
use openssl::rsa::Padding;
use openssl::sign::{RsaPssSaltlen, Verifier};
use openssl::x509::X509;
use serde::Deserialize;

use super::{unix_now, RequiredUser};
use crate::common::models::Role;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaAccessClaims {
    uid: u64,
    additional_roles: Vec<Role>,
    exp: u64,
}

pub(crate) struct CaAccessTokenVerifier {
    root_key: PKey<Public>,
}

impl CaAccessTokenVerifier {
    pub(crate) fn from_root_pem(root_pem: &[u8]) -> Result<Self, ErrorStack> {
        let root_key = X509::from_pem(root_pem)?.public_key()?;
        let verifier = Self { root_key };
        verifier.ps256()?;
        Ok(verifier)
    }

    fn ps256(&self) -> Result<Verifier<'_>, ErrorStack> {
        let mut verifier = Verifier::new(MessageDigest::sha256(), &self.root_key)?;
        verifier.set_rsa_padding(Padding::PKCS1_PSS)?;
        verifier.set_rsa_mgf1_md(MessageDigest::sha256())?;
        verifier.set_rsa_pss_saltlen(RsaPssSaltlen::custom(32))?;
        Ok(verifier)
    }

    pub(super) fn verify(&self, token: &str) -> Result<RequiredUser, actix_web::Error> {
        let invalid = || ErrorUnauthorized("invalid CA access token");
        let (signed, signature) = token.rsplit_once('.').ok_or_else(invalid)?;
        let (_, claims) = signed.split_once('.').ok_or_else(invalid)?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
        let mut verifier = self.ps256().map_err(|_| ErrorServiceUnavailable("CA authentication unavailable"))?;
        if !verifier.verify_oneshot(&signature, signed.as_bytes()).map_err(|_| invalid())? {
            return Err(invalid());
        }

        let claims = URL_SAFE_NO_PAD.decode(claims).map_err(|_| invalid())?;
        // Serde accepts positional arrays for structs; JWT claims must be an object.
        if !claims.trim_ascii_start().starts_with(b"{") {
            return Err(invalid());
        }
        let claims: CaAccessClaims = serde_json::from_slice(&claims).map_err(|_| invalid())?;
        if unix_now()? >= claims.exp {
            return Err(ErrorUnauthorized("token is expired"));
        }
        Ok(RequiredUser { uid: claims.uid, additional_roles: claims.additional_roles })
    }
}
