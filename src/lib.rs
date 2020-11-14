//! In Rust, values' destructors are automatically run when they go out of scope. However,
//! destructors can be expensive and so you may wish to defer running them until later, when your
//! program has some free time or memory usage is getting high. A bin allows you to put any number
//! of differently-typed values in it, and you can clear them all out, running their destructors,
//! whenever you want.
//!
//! # Example
//!
//! ```
//! let mut bin = drop_bin::Bin::new();
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

use std::cmp::max;
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::ptr;

type Destructor = unsafe fn(*mut ());

/// A bin.
///
/// It is cleared when it is dropped.
#[derive(Debug, Default)]
pub struct Bin<'a> {
    /// Pointers to the data and its destructors.
    destructors: Vec<(*mut (), Destructor)>,
    /// The backing storage behind the pointers in `destructors`. Each inner `Vec` never
    /// reallocates, and they double in size each time.
    data: Vec<Vec<MaybeUninit<u8>>>,
    invariant_over_lifetime_a: PhantomData<&'a mut &'a ()>,
}

impl<'a> Bin<'a> {
    // Associated constant as fn pointers aren't allowed in const fn
    const NEW: Self = Self {
        destructors: Vec::new(),
        data: Vec::new(),
        invariant_over_lifetime_a: PhantomData,
    };

    /// Create a new bin.
    #[must_use]
    pub const fn new() -> Self {
        Self::NEW
    }

    /// Add a value to the bin.
    pub fn add<T: 'a>(&mut self, value: T) {
        let align = mem::align_of::<T>();
        let size = mem::size_of::<T>();

        let value_ptr = if size > 0 {
            // The option borrows Self so I can't use combinators
            #[allow(clippy::option_if_let_else)]
            // Find a storage that has space for the value.
            let (storage, value_start_index) = if let Some(x) =
                self.data.iter_mut().find_map(|storage| {
                    let storage_end_ptr = storage.as_ptr() as usize + storage.len();
                    let padding = (align - storage_end_ptr % align) % align;

                    let value_start_index = storage.len().checked_add(padding)?;

                    if value_start_index.checked_add(size)? <= storage.capacity() {
                        Some((storage, value_start_index))
                    } else {
                        None
                    }
                }) {
                x
            } else {
                let capacity = max(
                    match size.checked_add(align) {
                        Some(x) => x,
                        None => return,
                    },
                    self.data.last().map_or(1024, |v| v.len().checked_mul(2).unwrap_or(v.len())),
                );
                self.data.push(Vec::with_capacity(capacity));
                let storage = self.data.last_mut().unwrap();
                let value_start_index = (align - storage.as_ptr() as usize % align) % align;
                (storage, value_start_index)
            };
            storage.resize(value_start_index + size, MaybeUninit::uninit());

            let value_ptr = &mut storage[value_start_index] as *mut MaybeUninit<u8>;
            assert_eq!(value_ptr as usize % align, 0);
            assert_eq!(storage.len() - value_start_index, size);

            unsafe {
                // SAFETY: We have mutable access to `data` and it is aligned.
                (value_ptr as *mut T).write(value);
            }

            value_ptr as *mut ()
        } else {
            align as *mut ()
        };

        let destructor: Destructor = unsafe {
            // SAFETY: `*mut T` can be soundly transmuted to `*mut ()`, and so `fn(*mut T)` can be
            // soundly transmuted to `fn(*mut ())`
            mem::transmute(ptr::drop_in_place::<T> as unsafe fn(*mut T))
        };

        self.destructors.push((value_ptr, destructor));
    }

    /// Clear the bin.
    pub fn clear(&mut self) {
        for (value, destructor) in self.destructors.drain(..) {
            unsafe {
                // SAFETY: `self.destructors` contains valid indices into `self.data`.
                // We use pointer arithmetic instead of indexing to avoid panicking when we drop
                // ZSTs (which are represented as an index 0).
                destructor(value as *mut ())
            }
        }

        for storage in &mut self.data {
            storage.clear();
        }
    }

    /// Get the size of the bin in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.data.iter().map(Vec::len).sum()
    }
}

impl<'a> Drop for Bin<'a> {
    fn drop(&mut self) {
        self.clear();
    }
}

unsafe impl<'a> Send for Bin<'a> {}
unsafe impl<'a> Sync for Bin<'a> {}

#[test]
fn test_bin() {
    use std::cell::Cell;

    #[cfg(test)]
    struct CallOnDrop<T: FnMut()>(T);
    #[cfg(test)]
    impl<T: FnMut()> Drop for CallOnDrop<T> {
        fn drop(&mut self) {
            self.0();
        }
    }

    let destructor_called = Cell::new(false);

    let mut bin = Bin::new();
    assert!(bin.destructors.is_empty());
    assert!(bin.data.is_empty());

    let val = CallOnDrop(|| destructor_called.set(true));
    bin.add(val);
    assert_eq!(bin.destructors.len(), 1);
    assert!(!destructor_called.get());

    bin.add(253_u8);
    assert_eq!(bin.destructors.len(), 2);
    assert_eq!(unsafe { *(bin.destructors[1].0 as *const u8) }, 253);

    bin.add(Box::new(6));
    assert_eq!(bin.destructors.len(), 3);
    assert!(!destructor_called.get());

    bin.clear();

    assert!(destructor_called.get());

    bin.clear();
}

#[test]
fn test_bin_zsts() {
    use std::marker::PhantomData;

    let mut bin = Bin::new();

    bin.add(());
    bin.add(());
    bin.add(PhantomData::<()>);
    bin.add(PhantomData::<Vec<i64>>);

    bin.clear();
}
