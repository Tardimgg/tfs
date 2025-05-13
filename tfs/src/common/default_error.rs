use std::fmt::Debug;
use log::error;

pub trait DefaultError<T> {

    fn default_res(self) -> Result<T, String>;
    fn default_logging_res(self, prefix: &str) -> Result<T, String>;

}

impl<T, E: ToString> DefaultError<T> for Result<T, E> {

    fn default_res(self) -> Result<T, String> {
        self.map_err(|v| v.to_string())
    }

    fn default_logging_res(self, prefix: &str) -> Result<T, String> {
        if let Err(e) = &self {
            error!("{prefix}{}", e.to_string());
        }
        self.default_res()
    }
}