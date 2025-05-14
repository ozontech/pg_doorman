//! A generic connection pool with async/await support.

use super::error::Error;

use super::config::Builder;
use super::config::{Config, InternalConfig, ShareConfig};
use super::spawn::spawn;
use super::conn::{ActiveConn, ConnState, IdleConn};

pub use async_trait::async_trait;
use futures_channel::mpsc::{self, Receiver, Sender};
use futures_util::lock::{Mutex, MutexGuard};
use futures_util::select;
use futures_util::FutureExt;
use futures_util::StreamExt;
use std::fmt;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Weak,
};
use std::time::{Duration, Instant};
use log::error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const CONNECTION_REQUEST_QUEUE_SIZE: usize = 10000;

#[async_trait]
/// A trait which provides connection-specific functionality.
pub trait Manager: Send + Sync + 'static {
    /// The connection type this manager deals with.
    type Connection: Send + 'static;
    /// The error type returned by `Connection`s.
    type Error: Send + Sync + 'static;

    /// Spawns a new asynchronous task.
    fn spawn_task<T>(&self, task: T)
    where
        T: Future + Send + 'static,
        T::Output: Send + 'static,
    {
        spawn(task);
    }

    /// Attempts to create a new connection.
    async fn connect(&self) -> Result<Self::Connection, Self::Error>;

    /// Determines if the connection is still connected to the database when check-out.
    ///
    /// A standard implementation would check if a simple query like `SELECT 1`
    /// succeeds.
    async fn check(&self, conn: Self::Connection) -> Result<Self::Connection, Self::Error>;

    /// *Quickly* determines a connection is still valid when check-in.
    #[inline]
    fn validate(&self, _conn: &mut Self::Connection) -> bool {
        true
    }
}

struct SharedPool<M: Manager> {
    config: ShareConfig,
    manager: M,
    internals: Mutex<PoolInternals<M::Connection>>,
    state: PoolState,
    semaphore: Arc<Semaphore>,
}

struct PoolInternals<C> {
    config: InternalConfig,
    free_conns: Vec<IdleConn<C>>,
    wait_duration: Duration,
    cleaner_ch: Option<Sender<()>>,
}

struct PoolState {
    num_open: Arc<AtomicU64>,
    max_lifetime_closed: AtomicU64,
    max_idle_closed: Arc<AtomicU64>,
    wait_count: AtomicU64,
}

impl<C> Drop for PoolInternals<C> {
    fn drop(&mut self) {
        log::debug!("Pool internal drop");
    }
}

/// A generic connection pool.
pub struct Pool<M: Manager>(Arc<SharedPool<M>>);

/// Returns a new `Pool` referencing the same state as `self`.
impl<M: Manager> Clone for Pool<M> {
    fn clone(&self) -> Self {
        Pool(self.0.clone())
    }
}

impl<M: Manager> fmt::Debug for Pool<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pool")
    }
}

/// Information about the state of a `Pool`.
#[derive(Debug)]
pub struct State {
    /// Maximum number of open connections to the database
    pub max_open: u64,

    // Pool Status
    /// The number of established connections both in use and idle.
    pub connections: u64,
    /// The number of connections currently in use.
    pub in_use: u64,
    /// The number of idle connections.
    pub idle: u64,

    // Counters
    /// The total number of connections waited for.
    pub wait_count: u64,
    /// The total time blocked waiting for a new connection.
    pub wait_duration: Duration,
    /// The total number of connections closed due to `max_idle`.
    pub max_idle_closed: u64,
    /// The total number of connections closed due to `max_lifetime`.
    pub max_lifetime_closed: u64,
}

impl<M: Manager> Drop for Pool<M> {
    fn drop(&mut self) {}
}

impl<M: Manager> Pool<M> {
    /// Creates a new connection pool with a default configuration.
    pub fn new(manager: M) -> Pool<M> {
        Pool::builder().build(manager)
    }

    /// Returns a builder type to configure a new pool.
    pub fn builder() -> Builder<M> {
        Builder::new()
    }

    /// Sets the maximum number of connections managed by the pool.
    ///
    /// 0 means unlimited, defaults to 10.
    pub async fn set_max_open_conns(&self, n: u64) {
        let mut internals = self.0.internals.lock().await;
        internals.config.max_open = n;
        if n > 0 && internals.config.max_idle > n {
            drop(internals);
            self.set_max_idle_conns(n).await;
        }
    }

    /// Sets the maximum idle connection count maintained by the pool.
    ///
    /// The pool will maintain at most this many idle connections
    /// at all times, while respecting the value of `max_open`.
    ///
    /// 0 means unlimited (limited only by `max_open`), defaults to 2.
    pub async fn set_max_idle_conns(&self, n: u64) {
        let mut internals = self.0.internals.lock().await;
        internals.config.max_idle =
            if internals.config.max_open > 0 && n > internals.config.max_open {
                internals.config.max_open
            } else {
                n
            };

        let max_idle = internals.config.max_idle as usize;
        // Treat max_idle == 0 as unlimited
        if max_idle > 0 && internals.free_conns.len() > max_idle {
            internals.free_conns.truncate(max_idle);
        }
    }

    /// Sets the maximum lifetime of connections in the pool.
    ///
    /// Expired connections may be closed lazily before reuse.
    ///
    /// None means reuse forever.
    /// Defaults to None.
    ///
    /// # Panics
    ///
    /// Panics if `max_lifetime` is the zero `Duration`.
    pub async fn set_conn_max_lifetime(&self, max_lifetime: Option<Duration>) {
        assert_ne!(
            max_lifetime,
            Some(Duration::from_secs(0)),
            "max_lifetime must be positive"
        );
        let mut internals = self.0.internals.lock().await;
        internals.config.max_lifetime = max_lifetime;
        if let Some(lifetime) = max_lifetime {
            match internals.config.max_lifetime {
                Some(prev) if lifetime < prev && internals.cleaner_ch.is_some() => {
                    // FIXME
                    let _ = internals.cleaner_ch.as_mut().unwrap().try_send(());
                }
                _ => (),
            }
        }

        if max_lifetime.is_some()
            && self.0.state.num_open.load(Ordering::Relaxed) > 0
            && internals.cleaner_ch.is_none()
        {
            let shared1 = Arc::downgrade(&self.0);
            let clean_rate = self.0.config.clean_rate;
            let (cleaner_ch_sender, cleaner_ch) = mpsc::channel(1);
            internals.cleaner_ch = Some(cleaner_ch_sender);
            self.0.manager.spawn_task(async move {
                connection_cleaner(shared1, cleaner_ch, clean_rate).await;
            });
        }
    }

    pub async fn clean_connections(&self) {
        let mut internals = self.0.internals.lock().await;
        if self.0.state.num_open.load(Ordering::Relaxed) > 0 && internals.cleaner_ch.is_none() {
            let shared1 = Arc::downgrade(&self.0);
            let clean_rate = self.0.config.clean_rate;
            let (cleaner_ch_sender, cleaner_ch) = mpsc::channel(1);
            internals.cleaner_ch = Some(cleaner_ch_sender);
            self.0.manager.spawn_task(async move {
                connection_cleaner(shared1, cleaner_ch, clean_rate).await;
            });
        }
    }

    pub(crate) fn new_inner(manager: M, config: Config) -> Self {
        let max_open = if config.max_open == 0 {
            CONNECTION_REQUEST_QUEUE_SIZE
        } else {
            config.max_open as usize
        };

        let (share_config, internal_config) = config.split();
        let internals = Mutex::new(PoolInternals {
            config: internal_config,
            free_conns: Vec::new(),
            wait_duration: Duration::from_secs(0),
            cleaner_ch: None,
        });

        let pool_state = PoolState {
            num_open: Arc::new(AtomicU64::new(0)),
            max_lifetime_closed: AtomicU64::new(0),
            wait_count: AtomicU64::new(0),
            max_idle_closed: Arc::new(AtomicU64::new(0)),
        };

        let shared = Arc::new(SharedPool {
            config: share_config,
            manager,
            internals,
            semaphore: Arc::new(Semaphore::new(max_open)),
            state: pool_state,
        });

        Pool(shared)
    }

    /// Returns a single connection by either opening a new connection
    /// or returning an existing connection from the connection pool. Conn will
    /// block until either a connection is returned or timeout.
    pub async fn get(&self) -> Result<Connection<M>, Error<M::Error>> {
        match self.0.config.get_timeout {
            Some(duration) => self.get_timeout(duration).await,
            None => self.inner_get_with_retries().await,
        }
    }

    /// Retrieves a connection from the pool, waiting for at most `timeout`
    ///
    /// The given timeout will be used instead of the configured connection
    /// timeout.
    pub async fn get_timeout(&self, duration: Duration) -> Result<Connection<M>, Error<M::Error>> {
        match tokio::time::timeout(duration, self.inner_get_with_retries()).await {
            Ok(result) => match result {
                Ok(conn) => Ok(conn),
                Err(err) => Err(err),
            },
            Err(err) => {
                error!("Get connection: {}", err);
                Err(Error::Timeout)
            }
        }
    }

    async fn inner_get_with_retries(&self) -> Result<Connection<M>, Error<M::Error>> {
        let mut try_times: u32 = 0;
        let config = &self.0.config;
        loop {
            try_times += 1;
            match self.get_connection().await {
                Ok(conn) => return Ok(conn),
                Err(Error::BadConn) => {
                    if try_times == config.max_bad_conn_retries {
                        return self.get_connection().await;
                    }
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn get_connection(&self) -> Result<Connection<M>, Error<M::Error>> {
        let c = self.get_or_create_conn().await?;

        let conn = Connection {
            pool: self.clone(),
            conn: Some(c),
        };

        Ok(conn)
    }

    async fn validate_conn(
        &self,
        internal_config: InternalConfig,
        conn: IdleConn<M::Connection>,
    ) -> Option<IdleConn<M::Connection>> {
        if conn.is_brand_new() {
            return Some(conn);
        }

        if conn.expired(internal_config.max_lifetime) {
            return None;
        }

        if conn.idle_expired(internal_config.max_idle_lifetime) {
            return None;
        }

        let (raw, split) = conn.split_raw();
        let checked_raw = self.0.manager.check(raw).await.ok()?;
        let checked = split.restore(checked_raw);
        Some(checked)
    }

    async fn get_or_create_conn(&self) -> Result<ActiveConn<M::Connection>, Error<M::Error>> {
        self.0.state.wait_count.fetch_add(1, Ordering::Relaxed);

        let semaphore = Arc::clone(&self.0.semaphore);
        let permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| Error::PoolClosed)?;

        self.0.state.wait_count.fetch_sub(1, Ordering::SeqCst);

        let mut internals = self.0.internals.lock().await;

        let conn = internals.free_conns.pop();
        let internal_config = internals.config.clone();
        drop(internals);

        if let Some(conn) = conn {
            if let Some(valid_conn) = self.validate_conn(internal_config, conn).await {
                return Ok(valid_conn.into_active(permit));
            }
        }

        self.open_new_connection(permit).await
    }

    async fn open_new_connection(
        &self,
        permit: OwnedSemaphorePermit,
    ) -> Result<ActiveConn<M::Connection>, Error<M::Error>> {
        match self.0.manager.connect().await {
            Ok(c) => {
                self.0.state.num_open.fetch_add(1, Ordering::Relaxed);
                let state = ConnState::new(
                    Arc::clone(&self.0.state.num_open),
                    Arc::clone(&self.0.state.max_idle_closed),
                );
                Ok(ActiveConn::new(c, permit, state))
            }
            Err(e) => Err(Error::Inner(e)),
        }
    }

    /// Returns information about the current state of the pool.
    /// It is better to use the metrics than this method, this method
    /// requires a lock on the internals
    pub async fn state(&self) -> State {
        let internals = self.0.internals.lock().await;
        let num_free_conns = internals.free_conns.len() as u64;
        let wait_duration = internals.wait_duration;
        let max_open = internals.config.max_open;
        drop(internals);
        State {
            max_open,

            connections: self.0.state.num_open.load(Ordering::Relaxed),
            in_use: self.0.state.num_open.load(Ordering::Relaxed) - num_free_conns,
            idle: num_free_conns,

            wait_count: self.0.state.wait_count.load(Ordering::Relaxed),
            wait_duration,
            max_idle_closed: self.0.state.max_idle_closed.load(Ordering::Relaxed),
            max_lifetime_closed: self.0.state.max_lifetime_closed.load(Ordering::Relaxed),
        }
    }
}

async fn recycle_conn<M: Manager>(
    shared: &Arc<SharedPool<M>>,
    mut conn: ActiveConn<M::Connection>,
) {
    if conn_still_valid(shared, &mut conn) {
        conn.set_brand_new(false);
        let internals = shared.internals.lock().await;
        put_idle_conn::<M>(internals, conn);
    }
}

fn conn_still_valid<M: Manager>(
    shared: &Arc<SharedPool<M>>,
    conn: &mut ActiveConn<M::Connection>,
) -> bool {
    if !shared.manager.validate(conn.as_raw_mut()) {
        return false;
    }
    true
}

fn put_idle_conn<M: Manager>(
    mut internals: MutexGuard<'_, PoolInternals<M::Connection>>,
    conn: ActiveConn<M::Connection>,
) {
    let idle_conn = conn.into_idle();
    // Treat max_idle == 0 as unlimited idle connections.
    if internals.config.max_idle == 0
        || internals.config.max_idle > internals.free_conns.len() as u64
    {
        internals.free_conns.push(idle_conn);
    }
}

async fn connection_cleaner<M: Manager>(
    shared: Weak<SharedPool<M>>,
    mut cleaner_ch: Receiver<()>,
    clean_rate: Duration,
) {
    let mut interval = tokio::time::interval(clean_rate);
    interval.tick().await;
    loop {
        select! {
            _ = interval.tick().fuse() => (),
            r = cleaner_ch.next().fuse() => match r{
                Some(()) => (),
                None=> return
            },
        }

        if !clean_connection(&shared).await {
            return;
        }
    }
}

async fn clean_connection<M: Manager>(shared: &Weak<SharedPool<M>>) -> bool {
    let shared = match shared.upgrade() {
        Some(shared) => shared,
        None => {
            return false;
        }
    };

    let mut internals = shared.internals.lock().await;
    if shared.state.num_open.load(Ordering::Relaxed) == 0 || internals.config.max_lifetime.is_none()
    {
        internals.cleaner_ch.take();
        return false;
    }

    let expired = Instant::now() - internals.config.max_lifetime.unwrap();
    let mut closing = vec![];

    let mut i = 0;

    loop {
        if i >= internals.free_conns.len() {
            break;
        }

        if internals.free_conns[i].created_at() < expired {
            let c = internals.free_conns.swap_remove(i);
            closing.push(c);
            continue;
        }
        i += 1;
    }
    drop(internals);

    shared
        .state
        .max_lifetime_closed
        .fetch_add(closing.len() as u64, Ordering::Relaxed);
    true
}

/// A smart pointer wrapping a connection.
pub struct Connection<M: Manager> {
    pool: Pool<M>,
    conn: Option<ActiveConn<M::Connection>>,
}

impl<M: Manager> Connection<M> {
    /// Returns true is the connection is newly established.
    pub fn is_brand_new(&self) -> bool {
        self.conn.as_ref().unwrap().is_brand_new()
    }

    /// Unwraps the raw database connection.
    pub fn into_inner(mut self) -> M::Connection {
        self.conn.take().unwrap().into_raw()
    }
}

impl<M: Manager> Drop for Connection<M> {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else {
            return;
        };

        let pool = Arc::clone(&self.pool.0);

        self.pool.0.manager.spawn_task(async move {
            recycle_conn(&pool, conn).await;
        });
    }
}

impl<M: Manager> Deref for Connection<M> {
    type Target = M::Connection;
    fn deref(&self) -> &Self::Target {
        self.conn.as_ref().unwrap().as_raw_ref()
    }
}

impl<M: Manager> DerefMut for Connection<M> {
    fn deref_mut(&mut self) -> &mut M::Connection {
        self.conn.as_mut().unwrap().as_raw_mut()
    }
}
