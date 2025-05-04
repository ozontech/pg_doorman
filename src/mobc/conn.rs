use std::{
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use tokio::sync::OwnedSemaphorePermit;

pub(crate) struct ActiveConn<C> {
    inner: C,
    state: ConnState,
    _permit: OwnedSemaphorePermit,
}

impl<C> ActiveConn<C> {
    pub(crate) fn new(inner: C, permit: OwnedSemaphorePermit, state: ConnState) -> ActiveConn<C> {
        Self {
            inner,
            state,
            _permit: permit,
        }
    }

    pub(crate) fn into_idle(self) -> IdleConn<C> {
        IdleConn {
            inner: self.inner,
            state: self.state,
        }
    }

    pub(crate) fn is_brand_new(&self) -> bool {
        self.state.brand_new
    }

    pub(crate) fn set_brand_new(&mut self, brand_new: bool) {
        self.state.brand_new = brand_new;
    }

    pub(crate) fn into_raw(self) -> C {
        self.inner
    }

    pub(crate) fn as_raw_ref(&self) -> &C {
        &self.inner
    }

    pub(crate) fn as_raw_mut(&mut self) -> &mut C {
        &mut self.inner
    }
}

pub(crate) struct IdleConn<C> {
    inner: C,
    state: ConnState,
}

impl<C> IdleConn<C> {
    pub(crate) fn is_brand_new(&self) -> bool {
        self.state.brand_new
    }

    pub(crate) fn into_active(self, permit: OwnedSemaphorePermit) -> ActiveConn<C> {
        ActiveConn::new(self.inner, permit, self.state)
    }

    pub(crate) fn created_at(&self) -> Instant {
        self.state.created_at
    }

    pub(crate) fn expired(&self, timeout: Option<Duration>) -> bool {
        timeout
            .and_then(|check_interval| {
                Instant::now()
                    .checked_duration_since(self.state.created_at)
                    .map(|dur_since| dur_since >= check_interval)
            })
            .unwrap_or(false)
    }

    pub(crate) fn idle_expired(&self, timeout: Option<Duration>) -> bool {
        timeout
            .and_then(|check_interval| {
                Instant::now()
                    .checked_duration_since(self.state.last_used_at)
                    .map(|dur_since| dur_since >= check_interval)
            })
            .unwrap_or(false)
    }

    pub(crate) fn split_raw(self) -> (C, ConnSplit<C>) {
        (self.inner, ConnSplit::new(self.state))
    }
}

pub(crate) struct ConnState {
    pub(crate) created_at: Instant,
    pub(crate) last_used_at: Instant,
    pub(crate) brand_new: bool,
    total_connections_open: Arc<AtomicU64>,
    total_connections_closed: Arc<AtomicU64>,
}

impl ConnState {
    pub(crate) fn new(
        total_connections_open: Arc<AtomicU64>,
        total_connections_closed: Arc<AtomicU64>,
    ) -> Self {
        Self {
            created_at: Instant::now(),
            last_used_at: Instant::now(),
            brand_new: true,
            total_connections_open,
            total_connections_closed,
        }
    }
}

impl Drop for ConnState {
    fn drop(&mut self) {
        self.total_connections_open.fetch_sub(1, Ordering::Relaxed);
        self.total_connections_closed
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct ConnSplit<C> {
    state: ConnState,
    _phantom: PhantomData<C>,
}

impl<C> ConnSplit<C> {
    fn new(state: ConnState) -> Self {
        Self {
            state,
            _phantom: PhantomData,
        }
    }

    pub(crate) fn restore(self, raw: C) -> IdleConn<C> {
        IdleConn {
            inner: raw,
            state: self.state,
        }
    }
}
