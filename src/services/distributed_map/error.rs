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
pub enum GetError {
    #[error("Invalid Id encoding: {0}")]
    MainlineDhtError(#[from] mainline::Error),
    #[error("Invalid Id encoding: {0}")]
    InvalidKey(#[from] KeyError),
    #[error("An unexpected value was received from dht")]
    InternalError
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
pub enum PutError {
    #[error("Invalid Id encoding: {0}")]
    InvalidKey(#[from] KeyError),
    #[error("Invalid ")]
    InvalidValue,
    #[error("Invalid ")]
    XZError,
    #[error("Invalid ")]
    AlreadyExist,
    #[error("Inval")]
    UnexpectedDhtState(#[from] serde_json::Error),
    #[error("Inval")]
    InternalError(#[from] GetError)
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
