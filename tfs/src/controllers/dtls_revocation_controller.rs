use std::sync::Arc;

use actix_web::{post, web, HttpRequest, HttpResponse, Scope};
use serde::Serialize;

use crate::common::exceptions::ApiException;
use crate::services::dtls_revocation_registry::{parse_detached_signature, DtlsRevocationRegistry, RevocationRegistryError, REVOCATION_SIGNATURE_HEADER};

const MAX_REVOCATION_JSON_SIZE: usize = 64 * 1024;

#[derive(Serialize)]
struct PublishResponse {
    sequence: i64,
}

#[post("/revocations")]
async fn publish(request: HttpRequest, body: web::Bytes, registry: web::Data<Arc<DtlsRevocationRegistry>>) -> Result<HttpResponse, ApiException> {
    if request.connection_info().scheme() != "https" {
        return Err(ApiException::NotFound);
    }
    if body.len() > MAX_REVOCATION_JSON_SIZE {
        return Err(ApiException::BadRequest("revocation document is too large".to_string()));
    }
    let signature = request
        .headers()
        .get(REVOCATION_SIGNATURE_HEADER)
        .ok_or_else(|| ApiException::Forbidden("detached signature is required".to_string()))?
        .to_str()
        .map_err(|_| ApiException::BadRequest("invalid signature header".to_string()))?;
    let signature = parse_detached_signature(signature.as_bytes()).map_err(api_error)?;
    let sequence = registry.publish(body.to_vec(), signature).await.map_err(api_error)?;
    Ok(HttpResponse::Ok().json(PublishResponse { sequence }))
}

fn api_error(error: RevocationRegistryError) -> ApiException {
    let message = error.to_string();
    match error {
        RevocationRegistryError::InvalidSignature => ApiException::Forbidden(message),
        RevocationRegistryError::InvalidSignatureEncoding
        | RevocationRegistryError::InvalidBinaryEncoding
        | RevocationRegistryError::InvalidDocument(_)
        | RevocationRegistryError::IssuerMismatch { .. }
        | RevocationRegistryError::OuterSequenceMismatch
        | RevocationRegistryError::ValueTooLarge { .. } => ApiException::BadRequest(message),
        RevocationRegistryError::UnexpectedSequence { .. }
        | RevocationRegistryError::SequenceConflict(_)
        | RevocationRegistryError::SequenceOverflow => ApiException::Conflict(message),
        RevocationRegistryError::DhtGet(_)
        | RevocationRegistryError::AdvancementFailed
        | RevocationRegistryError::DhtPut(_)
        | RevocationRegistryError::Mainline(_)
        | RevocationRegistryError::PublicationNotConfirmed(_) => {
            ApiException::ServiceUnavailable(message)
        }
        RevocationRegistryError::SnapshotNotApplied(_)
        | RevocationRegistryError::LocalArtifactNotInstalled(_)
        | RevocationRegistryError::MissingInitialArtifact
        | RevocationRegistryError::Codec(_)
        | RevocationRegistryError::Io(_)
        | RevocationRegistryError::InvalidRootCa(_)
        | RevocationRegistryError::Crypto(_)
        | RevocationRegistryError::Load { .. }
        | RevocationRegistryError::InvalidDhtKey(_) => ApiException::InternalError(message),
    }
}

pub(crate) fn config() -> Scope {
    web::scope("/security/dtls").service(publish)
}
