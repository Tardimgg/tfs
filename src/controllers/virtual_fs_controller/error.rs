use std::fmt::{Display, Formatter};
use actix_web::ResponseError;

#[derive(Debug)]
pub struct ApiException {
    message: String
}


impl Display for ApiException {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl ResponseError for ApiException {

}