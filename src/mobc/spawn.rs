use std::future::Future;
use tracing::instrument::WithSubscriber;
use tracing::Dispatch;

/// Spawns a new asynchronous task.
pub fn spawn<T>(task: T)
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    spawn_tokio(task);
}

pub fn spawn_tokio<T>(task: T)
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    let dispatcher = get_current_dispatcher();
    tokio::spawn(task.with_subscriber(dispatcher));
}

pub fn get_current_dispatcher() -> Dispatch {
    tracing::dispatcher::get_default(|current| current.clone())
}