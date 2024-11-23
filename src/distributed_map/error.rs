use thiserror::Error;

#[derive(Error, Debug)]
pub enum CreateError {
    MainlineDhtError(#[from] mainline::Error)
}

// impl From<mainline::Error> for CreateError {
//     fn from(value: mainline::Error) -> Self {
//         CreateError::MainlineDhtError(value.to_string())
//     }
// }

#[derive(Error, Debug)]
pub enum GetError {
    MainlineDhtError(#[from] mainline::Error),
    InvalidKey(#[from] KeyError)
}

// impl From<mainline::Error> for GetError {
//     fn from(value: mainline::Error) -> Self {
//         GetError::MainlineDhtError(value.to_string())
//     }
// }

#[derive(Error, Debug)]
pub enum KeyError {
}

#[derive(Error, Debug)]
pub enum PutError {
    InvalidKey(#[from] KeyError)
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
