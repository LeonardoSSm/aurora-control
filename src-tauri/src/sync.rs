//! Lock helpers that never propagate poisoning (audit B02).
//!
//! A panic while a lock is held leaves the mutex poisoned. The data guarded
//! here is always left in a valid state by construction (the scheduler ends
//! every attempt through `finish` or `abort`), so later readers recover the
//! guard instead of panicking in turn, which would take down the IPC thread or
//! a worker permanently.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `Condvar::wait_timeout` that recovers a poisoned guard.
pub fn wait_timeout<'a, T>(
    condvar: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: Duration,
) -> MutexGuard<'a, T> {
    match condvar.wait_timeout(guard, timeout) {
        Ok((guard, _)) => guard,
        Err(poisoned) => poisoned.into_inner().0,
    }
}

/// `Condvar::wait_timeout_while` that recovers a poisoned guard.
pub fn wait_timeout_while<'a, T>(
    condvar: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: Duration,
    condition: impl FnMut(&mut T) -> bool,
) -> MutexGuard<'a, T> {
    match condvar.wait_timeout_while(guard, timeout, condition) {
        Ok((guard, _)) => guard,
        Err(poisoned) => poisoned.into_inner().0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_panic_while_locked_does_not_poison_later_readers() {
        let shared = Arc::new(Mutex::new(7));
        let clone = Arc::clone(&shared);
        let result = std::thread::spawn(move || {
            let _guard = clone.lock().unwrap();
            panic!("worker failure");
        })
        .join();
        assert!(result.is_err());
        assert!(shared.is_poisoned());
        assert_eq!(*lock(&shared), 7);
    }
}
