//! FIFO serialization for local speech and language-model engine work.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::pipeline::CancelToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmitError {
    #[error("cancelled while waiting")]
    Cancelled,
    #[error("timed out waiting for the engine")]
    TimedOut,
}

#[derive(Default)]
struct State {
    busy: bool,
    next_ticket: u64,
    waiting: Vec<u64>,
}

impl State {
    fn head(&self) -> Option<u64> {
        self.waiting.first().copied()
    }

    fn remove(&mut self, ticket: u64) {
        self.waiting.retain(|waiting| *waiting != ticket);
    }
}

#[derive(Default)]
pub struct EngineScheduler {
    state: Mutex<State>,
    wake: Condvar,
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
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn waiting(&self) -> usize {
        self.lock().waiting.len()
    }

    pub fn acquire(&self, cancel: &CancelToken, deadline: Instant) -> Result<Permit<'_>, AdmitError> {
        let mut state = self.lock();
        if cancel.is_cancelled() {
            return Err(AdmitError::Cancelled);
        }
        if !state.busy && state.waiting.is_empty() {
            state.busy = true;
            return Ok(Permit { scheduler: self });
        }
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push(ticket);
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
        let s = Arc::new(EngineScheduler::new());
        let first = s.acquire(&CancelToken::default(), far()).unwrap();
        let (tx, rx) = mpsc::channel();
        let s2 = s.clone();
        let handle = thread::spawn(move || {
            let _p = s2.acquire(&CancelToken::default(), far()).unwrap();
            tx.send(()).unwrap();
        });
        wait_for_waiters(&s, 1);
        assert!(rx.recv_timeout(Duration::from_millis(60)).is_err(), "second permit granted while first held");
        drop(first);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn queued_jobs_run_in_arrival_order() {
        let s = Arc::new(EngineScheduler::new());
        let held = s.acquire(&CancelToken::default(), far()).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::new();
        for client in ["first", "second", "third"] {
            let s2 = s.clone();
            let tx = tx.clone();
            handles.push(thread::spawn(move || {
                let _p = s2.acquire(&CancelToken::default(), far()).unwrap();
                tx.send(client).unwrap();
                thread::sleep(Duration::from_millis(5));
            }));
            wait_for_waiters(&s, handles.len());
        }
        drop(held);
        let order: Vec<_> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap()).collect();
        assert_eq!(order, ["first", "second", "third"]);
        handles.into_iter().for_each(|h| h.join().unwrap());
    }

    #[test]
    fn waiting_respects_deadline_and_cancellation() {
        let s = EngineScheduler::new();
        let held = s.acquire(&CancelToken::default(), far()).unwrap();
        let started = Instant::now();
        let err = s.acquire(&CancelToken::default(), Instant::now() + Duration::from_millis(40)).err();
        assert_eq!(err, Some(AdmitError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(s.waiting(), 0);
        let cancel = CancelToken::default();
        cancel.cancel();
        assert_eq!(s.acquire(&cancel, far()).err(), Some(AdmitError::Cancelled));
        drop(held);
        assert!(s.acquire(&CancelToken::default(), far()).is_ok());
    }
}
