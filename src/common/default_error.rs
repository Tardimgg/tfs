use std::fmt::Debug;

pub trait DefaultError<T> {

    fn default_res(self) -> Result<T, String>;

}

impl<T, E: ToString> DefaultError<T> for Result<T, E> {

    fn default_res(self) -> Result<T, String> {
        self.map_err(|v| v.to_string())
    }
}