use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;
use crate::common::models::TokenType;

#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct UserToken {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: TokenType
}

#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct NewUser {
    pub uid: u64,
    pub token: UserToken
}