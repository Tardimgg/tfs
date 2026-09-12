use std::error::Error;
use thiserror::Error;
use ed25519_dalek::SignatureError;

#[derive(Error, Debug)]
pub enum DhtCodecError {
    #[error("failed to decode protobuf DHT value: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),
    #[error("failed to compress or decompress DHT value: {0}")]
    Compression(#[from] std::io::Error),
    #[error("unsupported DHT value version: {0}")]
    UnsupportedVersion(u32),
    #[error("missing protobuf field: {0}")]
    MissingField(&'static str),
    #[error("invalid protobuf DHT value: {0}")]
    InvalidValue(String),
    #[error("protobuf DHT record is too large before compression: {size} bytes, maximum is {max}")]
    UncompressedValueTooLarge { size: usize, max: usize },
    #[error("stored DHT value is too large: {size} bytes, maximum is {max}")]
    RecordEnvelopeTooLarge { size: usize, max: usize },
    #[error("unexpected DHT value type: expected {expected}, found {actual}")]
    UnexpectedValueType {
        expected: &'static str,
        actual: &'static str
    }
}

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
    InternalError(String),
    #[error(transparent)]
    Codec(#[from] DhtCodecError)
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
    #[error("encoded DHT value is too large: {size} bytes, maximum is {max}")]
    ValueTooLarge { size: usize, max: usize },
    #[error("DHT sequence number cannot be incremented: {0}")]
    SequenceOverflow(i64),
    #[error(transparent)]
    Codec(#[from] DhtCodecError),
    #[error(transparent)]
    Mainline(#[from] mainline::Error),
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
