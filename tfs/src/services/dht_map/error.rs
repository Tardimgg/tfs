use std::error::Error;
use thiserror::Error;
use ed25519_dalek::SignatureError;

#[derive(Error, Debug)]
pub enum CreateError {
    #[error("Invalid Id encoding: {0}")]
    MainlineDhtError(#[from] mainline::Error)
}

// impl From<mainline::Error> for CreateError {
//     fn from(value: mainline::Error) -> Self {
//         CreateError::MainlineDhtError(value.to_string())
//     }
// }

#[derive(Error, Debug)]
pub enum DhtGetError {
    #[error("Invalid Id encoding: {0}")]
    MainlineDhtError(#[from] mainline::Error),
    #[error("Invalid Id encoding: {0}")]
    InvalidKey(#[from] KeyError),
    #[error("An unexpected value was received from dht: {0}")]
    InternalError(String)
}

impl From<String> for DhtGetError {
    fn from(value: String) -> Self {
        DhtGetError::InternalError(value)
    }
}

// impl From<mainline::Error> for GetError {
//     fn from(value: mainline::Error) -> Self {
//         GetError::MainlineDhtError(value.to_string())
//     }
// }

#[derive(Error, Debug)]
pub enum KeyError {
    #[error("Invalid Id encoding: {0}")]
    HashingErr(#[from] std::io::Error),
    #[error("Invalid Id encoding: {0}")]
    InvalidKey(#[from] SignatureError)
}

#[derive(Error, Debug)]
pub enum DhtPutError {
    #[error("Invalid Id encoding: {0}")]
    InvalidKey(#[from] KeyError),
    #[error("Invalid ")]
    InvalidValue,
    #[error("Invalid ")]
    XZError,
    #[error("Invalid ")]
    AlreadyExist,
    #[error("Invallid dht state {0}")]
    UnexpectedDhtState(#[from] serde_json::Error),
    #[error("invalid internal state: {0}")]
    InternalError(String)
}

impl From<String> for DhtPutError {
    fn from(value: String) -> Self {
        DhtPutError::InternalError(value)
    }
}


// impl From<KeyError> for PutError {
//     fn from(value: KeyError) -> Self {
//         PutError::InvalidKey(value.to_string())
//     }
// }


// impl From<KeyError> for GetError {
//     fn from(value: KeyError) -> Self {
//         GetError::InvalidKey(value.to_string())
//     }
// }
