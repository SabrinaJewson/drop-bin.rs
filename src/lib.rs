//! In Rust, values' destructors are automatically run when they go out of scope. However,
//! destructors can be expensive and so you may wish to defer running them until later, when your
//! program has some free time or memory usage is getting high. A bin allows you to put any number
//! of differently-typed values in it, and you can clear them all out, running their destructors,
//! whenever you want.
//!
//! # Example
//!
//! ```
//! let bin = drop_bin::Bin::new();
//!
//! let some_data = "Hello World!".to_owned();
//! bin.add(some_data);
//! // `some_data`'s destructor is not run.
//!
//! bin.clear();
//! // `some_data`'s destructor has been run.
//! ```
#![warn(
    clippy::pedantic,
    rust_2018_idioms,
    missing_docs,
    unused_qualifications
)]

use std::sync::atomic::{self, AtomicBool};

use try_rwlock::TryRwLock;

mod concurrent_list;
use concurrent_list::ConcurrentList;

mod concurrent_slice;
use concurrent_slice::ConcurrentSlice;

mod concurrent_vec;
use concurrent_vec::ConcurrentVec;

mod inner;
use inner::Inner;

/// A container that holds values for later destruction.
///
/// It is automatically cleared when it is dropped.
#[derive(Debug, Default)]
pub struct Bin<'a> {
    /// The inner data of the bin. If this is locked for writing, the bin is being cleared.
    inner: TryRwLock<Inner<'a>>,
    /// Whether the bin needs to be cleared.
    clear: AtomicBool,
}

impl<'a> Bin<'a> {
    /// Create a new bin.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: TryRwLock::new(Inner::new()),
            clear: AtomicBool::new(false),
        }
    }

    /// Add a value to the bin.
    ///
    /// This may drop the value immediately, but will attempt to store it so that it can be dropped
    /// later.
    pub fn add<T: Send + 'a>(&self, value: T) {
        if let Some(inner) = self.inner.try_read() {
            inner.add(value);
        } else {
            // Just drop the value if the bin is being cleared.
        }

        self.try_clear();
    }

    /// Clear the bin, dropping all values that have been previously added to it.
    ///
    /// This may not clear the bin immediately if another thread is currently adding a value to the
    /// bin.
    pub fn clear(&self) {
        self.clear.store(true, atomic::Ordering::Relaxed);

        self.try_clear();
    }

    /// Attempt to the clear the bin.
    fn try_clear(&self) {
        if self.clear.load(atomic::Ordering::Relaxed) {
            if let Some(mut inner) = self.inner.try_write() {
                self.clear.store(false, atomic::Ordering::Relaxed);
                inner.clear();
            }
        }
    }

    /// Get the size of the bin in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.inner.try_read().map_or(0, |inner| inner.size())
    }
}

impl<'a> Drop for Bin<'a> {
    fn drop(&mut self) {
        self.inner.get_mut().clear();
    }
}

#[test]
fn test_clear() {
    use std::sync::atomic::Ordering::SeqCst;

    let destructor_called = AtomicBool::new(false);

    let bin = Bin::new();

    bin.add(CallOnDrop(
        || assert!(!destructor_called.swap(true, SeqCst)),
    ));
    assert!(!destructor_called.load(SeqCst));

    bin.clear();
    assert!(destructor_called.load(SeqCst));
}

#[cfg(test)]
fn assert_thread_safe<T: Send + Sync>() {}

#[test]
fn test_thread_safe() {
    assert_thread_safe::<Bin<'_>>();
}

#[cfg(test)]
struct CallOnDrop<T: FnMut()>(T);
#[cfg(test)]
impl<T: FnMut()> Drop for CallOnDrop<T> {
    fn drop(&mut self) {
        self.0();
    }
}
