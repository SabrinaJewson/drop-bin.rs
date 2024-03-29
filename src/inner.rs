use crate::ConcurrentList;
use crate::ConcurrentVec;
use std::cmp::max;
use std::marker::PhantomData;
use std::mem;
use std::mem::MaybeUninit;
use std::ptr;
use try_mutex::TryMutex;

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
    invariant_over_lifetime_a: PhantomData<fn(&'a ()) -> &'a ()>,
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
    pub(crate) const fn new() -> Self {
        Self {
            destructors: ConcurrentVec::new(),
            data: ConcurrentList::new(),
            invariant_over_lifetime_a: PhantomData,
        }
    }

    /// Add the given value to the bin.
    pub(crate) fn add<T: Send + 'a>(&self, value: T) {
        let value_ptr = match self.store(value) {
            Some(value_ptr) => value_ptr,
            None => return,
        };

        let destructor: Destructor = unsafe {
            // SAFETY: `*mut T` can be soundly transmuted to `*mut ()`, and so `fn(*mut T)` can be
            // soundly transmuted to `fn(*mut ())`
            mem::transmute::<unsafe fn(*mut T), fn(*mut ())>(ptr::drop_in_place::<T>)
        };

        self.destructors.push((value_ptr.cast::<()>(), destructor));
    }

    /// Store the given value in the bin.
    ///
    /// Returns a pointer to the value, or `None` if it failed.
    fn store<T: Send + 'a>(&self, value: T) -> Option<*mut T> {
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
                    })
            {
                unsafe {
                    // SAFETY: We have checked that there is enough space to store
                    // `value_start_index + size` bytes, and the inner type is MaybeUninit.
                    storage.set_len(value_start_index + size);
                }

                let value_ptr = <*mut MaybeUninit<u8>>::cast::<T>(&mut storage[value_start_index]);
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
            mem::forget(value);

            // We can use a dangling pointer for zero sized types, as long as it's property
            // aligned and non-null.
            Some(align as *mut T)
        }
    }

    /// Add a storage that contains the given value.
    ///
    /// Returns a pointer to the value, or `None` if it failed.
    fn add_storage<T: Send + 'a>(&self, value: T) -> Option<*mut T> {
        let size = mem::size_of::<T>();
        let align = mem::align_of::<T>();

        // The capacity of the storage
        let capacity = max(
            size.checked_add(align)?,
            self.data.head().map_or(
                // The initial storage capacity will be 1024 bytes
                1024,
                // Storage capacity will double after that
                |s| s.capacity.checked_mul(2).unwrap_or(s.capacity),
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
        let value_ptr = <*mut MaybeUninit<u8>>::cast::<T>(&mut bytes[value_start_index]);
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
                destructor(value.cast::<()>());
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

#[cfg(test)]
mod tests {
    use crate::inner::Inner;
    use crate::test_util::assert_thread_safe;
    use crate::test_util::CallOnDrop;
    use std::cell::Cell;
    use std::marker::PhantomData;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering::SeqCst;

    #[test]
    fn bin() {
        let destructor_called = AtomicBool::new(false);

        let mut bin = Inner::new();
        assert!(bin.destructors.is_empty());
        assert!(bin.data.is_empty());

        let val = CallOnDrop(|| assert!(!destructor_called.swap(true, SeqCst)));
        bin.add(val);
        assert_eq!(bin.destructors.len(), 1);
        assert!(!destructor_called.load(SeqCst));

        bin.add(253_u16);
        assert_eq!(bin.destructors.len(), 2);
        assert_eq!(
            unsafe { *(bin.destructors.iter_assume_init_mut().next().unwrap().0 as *const u16) },
            253
        );

        bin.add(Box::new(6));
        assert_eq!(bin.destructors.len(), 3);
        assert!(!destructor_called.load(SeqCst));

        bin.clear();

        assert!(destructor_called.load(SeqCst));

        bin.clear();
    }

    #[test]
    fn bin_zsts() {
        thread_local! {
            static DESTRUCTOR_CALLED: Cell<bool> = Cell::new(false);
        }

        struct Zst;
        impl Drop for Zst {
            fn drop(&mut self) {
                assert!(!DESTRUCTOR_CALLED.with(Cell::get));
                DESTRUCTOR_CALLED.with(|cell| cell.set(true));
            }
        }

        let mut bin = Inner::new();

        bin.add(());
        bin.add(());
        bin.add(PhantomData::<()>);
        bin.add(PhantomData::<Vec<i64>>);
        bin.add(Zst);

        assert!(!DESTRUCTOR_CALLED.with(Cell::get));

        bin.clear();

        assert!(DESTRUCTOR_CALLED.with(Cell::get));

        DESTRUCTOR_CALLED.with(|cell| cell.set(false));
    }

    #[test]
    fn thread_safe() {
        assert_thread_safe::<Inner<'_>>();
    }
}
