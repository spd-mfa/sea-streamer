use futures::future::{ready, Ready};
use std::future::Future;

#[derive(Debug)]
pub struct JoinError;

pub fn spawn_task<F, T>(_: F) -> Ready<Result<T, JoinError>>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    ready(Err(JoinError))
}

pub fn spawn_blocking<F, T>(_: F) -> Ready<Result<T, JoinError>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    ready(Err(JoinError))
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Please enable a runtime")
    }
}

impl std::error::Error for JoinError {}
