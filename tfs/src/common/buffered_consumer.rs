use std::fmt::{Debug, Display, Formatter, Pointer};
use std::future::Future;
use thiserror::Error;

pub struct BufferedConsumer<T> {
    source: fn() -> Option<T>
}

enum ConsumerError{
    BufferIsFull
}


impl<T> BufferedConsumer<T> {

    // pub fn with_source<S, Fut>(source: S) -> Self
    // where
    //     S: FnMut() -> Fut,
    //     Fut: Future<Output = Option<T>>
    // {
    //     BufferedConsumer {
    //         source
    //     }
    // }


    pub fn pop_front() -> Option<T> {
        None
    }
}