//! Single-slot engine scheduler: local hotkey jobs run before paired devices, which run before tool callers.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::pipeline::CancelToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Local,
    Device,
    Tool,
}

#[derive(Debug, Clone)]
pub struct SchedulerLimits {
    pub max_waiting: usize,
    pub max_waiting_per_client: usize,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self { max_waiting: 8, max_waiting_per_client: 2 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmitError {
    #[error("the engine queue is full")]
    QueueFull,
    #[error("this client already has the maximum number of queued requests")]
    ClientQueueFull,
    #[error("cancelled while waiting")]
    Cancelled,
    #[error("timed out waiting for the engine")]
    TimedOut,
}

struct Waiter {
    ticket: u64,
    priority: Priority,
    client: String,
}

#[derive(Default)]
struct State {
    busy: bool,
    next_ticket: u64,
    waiting: Vec<Waiter>,
}

impl State {
    fn head(&self) -> Option<u64> {
        self.waiting.iter().min_by_key(|w| (w.priority, w.ticket)).map(|w| w.ticket)
    }

    fn remove(&mut self, ticket: u64) {
        self.waiting.retain(|w| w.ticket != ticket);
    }
}

pub struct EngineScheduler {
    state: Mutex<State>,
    wake: Condvar,
    limits: SchedulerLimits,
}

/// Holding a permit is the only way to use the speech and language engines.
pub struct Permit<'a> {
    scheduler: &'a EngineScheduler,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.scheduler.lock().busy = false;
        self.scheduler.wake.notify_all();
    }
}

// CancelToken has no wake-up hook, so waiters re-check it at this interval.
const POLL: Duration = Duration::from_millis(25);

impl EngineScheduler {
    pub fn new(limits: SchedulerLimits) -> Self {
        Self { state: Mutex::default(), wake: Condvar::new(), limits }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn waiting(&self) -> usize {
        self.lock().waiting.len()
    }

    /// Local jobs bypass the queue caps: the desktop already allows only one at a time.
    pub fn acquire(&self, priority: Priority, client: &str, cancel: &CancelToken, deadline: Instant) -> Result<Permit<'_>, AdmitError> {
        let mut state = self.lock();
        if cancel.is_cancelled() {
            return Err(AdmitError::Cancelled);
        }
        if !state.busy && state.waiting.is_empty() {
            state.busy = true;
            return Ok(Permit { scheduler: self });
        }
        if priority != Priority::Local {
            if state.waiting.len() >= self.limits.max_waiting {
                return Err(AdmitError::QueueFull);
            }
            if state.waiting.iter().filter(|w| w.client == client).count() >= self.limits.max_waiting_per_client {
                return Err(AdmitError::ClientQueueFull);
            }
        }
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push(Waiter { ticket, priority, client: client.to_owned() });
        loop {
            let exit = if cancel.is_cancelled() {
                Some(AdmitError::Cancelled)
            } else if Instant::now() >= deadline {
                Some(AdmitError::TimedOut)
            } else {
                None
            };
            if let Some(error) = exit {
                state.remove(ticket);
                drop(state);
                self.wake.notify_all();
                return Err(error);
            }
            if !state.busy && state.head() == Some(ticket) {
                state.remove(ticket);
                state.busy = true;
                return Ok(Permit { scheduler: self });
            }
            let wait = deadline.saturating_duration_since(Instant::now()).min(POLL);
            state = self.wake.wait_timeout(state, wait).unwrap_or_else(|p| p.into_inner()).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    fn wait_for_waiters(s: &EngineScheduler, n: usize) {
        let until = Instant::now() + Duration::from_secs(5);
        while s.waiting() != n {
            assert!(Instant::now() < until, "expected {n} waiters");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn only_one_permit_at_a_time_and_release_admits_next() {
        let s = Arc::new(EngineScheduler::new(SchedulerLimits::default()));
        let first = s.acquire(Priority::Device, "a", &CancelToken::default(), far()).unwrap();
        let (tx, rx) = mpsc::channel();
        let s2 = s.clone();
        let handle = thread::spawn(move || {
            let _p = s2.acquire(Priority::Device, "b", &CancelToken::default(), far()).unwrap();
            tx.send(()).unwrap();
        });
        wait_for_waiters(&s, 1);
        assert!(rx.recv_timeout(Duration::from_millis(60)).is_err(), "second permit granted while first held");
        drop(first);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn local_jobs_jump_ahead_of_queued_remote_jobs() {
        let s = Arc::new(EngineScheduler::new(SchedulerLimits::default()));
        let held = s.acquire(Priority::Tool, "t", &CancelToken::default(), far()).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::new();
        for (priority, client) in [(Priority::Tool, "t2"), (Priority::Device, "d"), (Priority::Local, "local")] {
            let s2 = s.clone();
            let tx = tx.clone();
            handles.push(thread::spawn(move || {
                let _p = s2.acquire(priority, client, &CancelToken::default(), far()).unwrap();
                tx.send(client).unwrap();
                thread::sleep(Duration::from_millis(5));
            }));
            wait_for_waiters(&s, handles.len());
        }
        drop(held);
        let order: Vec<_> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap()).collect();
        assert_eq!(order, ["local", "d", "t2"]);
        handles.into_iter().for_each(|h| h.join().unwrap());
    }

    #[test]
    fn queue_caps_apply_to_remote_clients_only() {
        let limits = SchedulerLimits { max_waiting: 3, max_waiting_per_client: 1 };
        let s = Arc::new(EngineScheduler::new(limits));
        let held = s.acquire(Priority::Device, "x", &CancelToken::default(), far()).unwrap();
        let cancel = CancelToken::default();
        let mut handles = Vec::new();
        for client in ["a", "b"] {
            let (s2, cancel) = (s.clone(), cancel.clone());
            handles.push(thread::spawn(move || s2.acquire(Priority::Device, client, &cancel, far()).map(|_| ())));
            wait_for_waiters(&s, handles.len());
        }
        assert_eq!(s.acquire(Priority::Device, "a", &cancel, far()).err(), Some(AdmitError::ClientQueueFull));
        let (s3, c3) = (s.clone(), cancel.clone());
        handles.push(thread::spawn(move || s3.acquire(Priority::Tool, "c", &c3, far()).map(|_| ())));
        wait_for_waiters(&s, 3);
        assert_eq!(s.acquire(Priority::Device, "d", &cancel, far()).err(), Some(AdmitError::QueueFull));
        let (s4, c4) = (s.clone(), cancel.clone());
        handles.push(thread::spawn(move || s4.acquire(Priority::Local, "local", &c4, far()).map(|_| ())));
        wait_for_waiters(&s, 4);
        cancel.cancel();
        for h in handles {
            assert_eq!(h.join().unwrap(), Err(AdmitError::Cancelled));
        }
        assert_eq!(s.waiting(), 0);
        drop(held);
    }

    #[test]
    fn waiting_respects_deadline_and_cancellation() {
        let s = EngineScheduler::new(SchedulerLimits::default());
        let held = s.acquire(Priority::Device, "x", &CancelToken::default(), far()).unwrap();
        let started = Instant::now();
        let err = s.acquire(Priority::Device, "y", &CancelToken::default(), Instant::now() + Duration::from_millis(40)).err();
        assert_eq!(err, Some(AdmitError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(s.waiting(), 0);
        let cancel = CancelToken::default();
        cancel.cancel();
        assert_eq!(s.acquire(Priority::Local, "z", &cancel, far()).err(), Some(AdmitError::Cancelled));
        drop(held);
        assert!(s.acquire(Priority::Device, "y", &CancelToken::default(), far()).is_ok());
    }
}
