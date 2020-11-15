use std::mem::{self, MaybeUninit};
use std::marker::PhantomData;
use std::cmp::max;
use std::ptr;

use try_mutex::TryMutex;

use crate::{ConcurrentVec, ConcurrentList};

type Destructor = unsafe fn(*mut ());

/// The inner data of a bin.
///
/// Unlike `Bin`, this cannot be cleared concurrently.
#[derive(Debug, Default)]
pub(crate) struct Inner<'a> {
    /// Pointers to the data and its destructors.
    destructors: ConcurrentVec<(*mut (), Destructor)>,
    /// The linked list of backing storage behind the pointers in `destructors`.
    data: ConcurrentList<Storage>,
    invariant_over_lifetime_a: PhantomData<&'a mut &'a ()>,
}

/// A segment of backing storage.
#[derive(Debug, Default)]
struct Storage {
    /// The bytes of data this element contains. This `Vec` must never reallocate.
    bytes: TryMutex<Vec<MaybeUninit<u8>>>,
    /// The capacity of the above `Vec`. This is stored separately so it can be accessed without
    /// locking the `TryMutex` as it doesn't change.
    capacity: usize,
}

impl<'a> Inner<'a> {
    // Associated constant as fn pointers and mutable refs aren't allowed in const fn
    #[allow(clippy::declare_interior_mutable_const)]
    const NEW: Self = Self {
        destructors: ConcurrentVec::new(),
        data: ConcurrentList::new(),
        invariant_over_lifetime_a: PhantomData,
    };
    pub(crate) const fn new() -> Self {
        Self::NEW
    }

    /// Add the given value to the bin.
    pub(crate) fn add<T: 'a>(&self, value: T) {
        let value_ptr = match self.store(value) {
            Some(value_ptr) => value_ptr,
            None => return,
        };

        let destructor: Destructor = unsafe {
            // SAFETY: `*mut T` can be soundly transmuted to `*mut ()`, and so `fn(*mut T)` can be
            // soundly transmuted to `fn(*mut ())`
            mem::transmute(ptr::drop_in_place::<T> as unsafe fn(*mut T))
        };

        self.destructors.push((value_ptr as *mut (), destructor));
    }

    /// Store the given value in the bin.
    ///
    /// Returns a pointer to the value, or `None` if it failed.
    fn store<T>(&self, value: T) -> Option<*mut T> {
        let size = mem::size_of::<T>();
        let align = mem::align_of::<T>();

        if size > 0 {
            // Attempt to reuse an existing storage for the value.
            if let Some((mut storage, value_start_index)) =
                // Find a storage that has space for the value.
                self.data.iter().find_map(|storage| {
                // If the storage is being used, just ignore it. We could keep on looping until
                // we've made sure that none of the storages have space for the value, but the
                // cost is only a few bytes in some scenarios.
                let storage = storage.bytes.try_lock()?;

                let storage_end_ptr = storage.as_ptr() as usize + storage.len();
                let padding = (align - storage_end_ptr % align) % align;

                let value_start_index = storage.len().checked_add(padding)?;

                if value_start_index.checked_add(size)? <= storage.capacity() {
                    Some((storage, value_start_index))
                } else {
                    None
                }
            }) {
                unsafe {
                    // SAFETY: We have checked that there is enough space to store
                    // `value_start_index + size` bytes, and the inner type is MaybeUninit.
                    storage.set_len(value_start_index + size);
                }

                let value_ptr = &mut storage[value_start_index] as *mut MaybeUninit<u8> as *mut T;
                unsafe {
                    // SAFETY: We have mutable access to `storage` and it is aligned.
                    value_ptr.write(value);
                }
                Some(value_ptr)
            } else {
                // Fall back to creating a new storage.
                self.add_storage(value)
            }
        } else {
            // We can use a dangling pointer for zero sized types, as long as it's property
            // aligned and non-null.
            Some(align as *mut T)
        }
    }

    /// Add a storage that contains the given value.
    ///
    /// Returns a pointer to the value, or `None` if it failed.
    fn add_storage<T>(&self, value: T) -> Option<*mut T> {
        let size = mem::size_of::<T>();
        let align = mem::align_of::<T>();

        // The capacity of the storage
        let capacity = max(
            size.checked_add(align)?,
            self.data.head().map_or(
                // The initial storage capacity will be 1024 bytes
                1024,
                // Storage capacity will double after that
                |s| {
                    s.capacity
                        .checked_mul(2)
                        .unwrap_or(s.capacity)
                },
            ),
        );
        let mut bytes = Vec::with_capacity(capacity);
        // Get the index into `bytes` at which the value starts to make sure it has the correct
        // alignment
        let value_start_index = (align - bytes.as_ptr() as usize % align) % align;
        unsafe {
            // SAFETY: We have allocated enough space to store `size + align` bytes, and the inner
            // type is MaybeUninit.
            bytes.set_len(value_start_index + size);
        }
        let value_ptr = &mut bytes[value_start_index] as *mut MaybeUninit<u8> as *mut T;
        unsafe {
            // SAFETY: We have mutable access to `bytes` and it is aligned.
            value_ptr.write(value);
        }

        let storage = Storage {
            bytes: TryMutex::new(bytes),
            capacity,
        };

        self.data.push(storage);
        Some(value_ptr)
    }

    /// Clear the bin.
    pub(crate) fn clear(&mut self) {
        for (value, destructor) in std::mem::take(&mut self.destructors).into_iter() {
            unsafe {
                // SAFETY: `self.destructors` contains valid indices into `self.data`.
                // We use pointer arithmetic instead of indexing to avoid panicking when we drop
                // ZSTs (which are represented as an index 0).
                destructor(value as *mut ())
            }
        }

        for storage in self.data.iter_mut() {
            storage.bytes.get_mut().clear();
        }
    }

    /// Get the size of the bin in bytes.
    pub(crate) fn size(&self) -> usize {
        self.data.iter().map(|s| s.capacity).sum()
    }
}

#[test]
fn test_bin() {
    let destructor_called = std::cell::Cell::new(false);

    let mut bin = Inner::new();
    assert!(bin.destructors.is_empty());
    assert!(bin.data.is_empty());

    let val = crate::CallOnDrop(|| destructor_called.set(true));
    bin.add(val);
    assert_eq!(bin.destructors.len(), 1);
    assert!(!destructor_called.get());

    bin.add(253_u16);
    assert_eq!(bin.destructors.len(), 2);
    assert_eq!(unsafe { *(bin.destructors.iter_mut().next().unwrap().0 as *const u16) }, 253);

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

    let mut bin = Inner::new();

    bin.add(());
    bin.add(());
    bin.add(PhantomData::<()>);
    bin.add(PhantomData::<Vec<i64>>);

    bin.clear();
}

#[test]
fn test_thread_safe() {
    crate::assert_thread_safe::<Inner<'_>>();
}
