//! Synchronization policy for recoverable in-process state.

use std::sync::{Mutex, MutexGuard};

/// Lock a standard mutex without cascading one worker panic into every later
/// request that touches the same state.
///
/// Rust poisons a mutex when a thread unwinds while holding it. Loca never
/// treats that flag as authorization or persistence state: SQLite transactions
/// roll back while unwinding, and every guarded value remains memory-safe. We
/// therefore report the exact call site, recover the guarded value, and clear
/// the poison flag. This is a deliberate degraded-mode policy; silently
/// converting these locks to a non-poisoning mutex would hide the incident,
/// while `unwrap()` would turn it into a repeatable service-wide panic.
pub(crate) trait RecoverMutex<T: ?Sized> {
    #[track_caller]
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> RecoverMutex<T> for Mutex<T> {
    #[track_caller]
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let caller = std::panic::Location::caller();
                tracing::error!(
                    file = caller.file(),
                    line = caller.line(),
                    "recovering a poisoned state mutex after an earlier panic"
                );
                let guard = poisoned.into_inner();
                self.clear_poison();
                guard
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RecoverMutex;
    use std::sync::{Arc, Mutex};

    #[test]
    fn poisoned_mutex_is_recovered_and_cleared() {
        let value = Arc::new(Mutex::new(7));
        let worker_value = Arc::clone(&value);
        let worker = std::thread::spawn(move || {
            let _guard = worker_value.lock_or_recover();
            panic!("intentional poison for recovery policy test");
        });
        assert!(worker.join().is_err());
        assert!(value.is_poisoned());

        *value.lock_or_recover() = 9;

        assert!(!value.is_poisoned());
        assert_eq!(*value.lock_or_recover(), 9);
    }
}
