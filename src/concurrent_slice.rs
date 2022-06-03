use std::cell::UnsafeCell;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::mem;
use std::mem::MaybeUninit;
use std::sync::atomic;
use std::sync::atomic::AtomicUsize;

/// A concurrent append-only boxed slice.
pub struct ConcurrentSlice<T> {
    data: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// The length up to which `data` is initialized.
    len: AtomicUsize,
}

impl<T> ConcurrentSlice<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            data: (0..capacity)
                .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                .collect(),
            len: AtomicUsize::new(0),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        self.data.len()
    }
    pub(crate) fn len(&self) -> usize {
        self.len.load(atomic::Ordering::Relaxed)
    }

    // This is safe because this container cannot be immutably iterated over
    pub(crate) fn push(&self, value: T) -> Result<&mut T, T> {
        let old_len = match self.len.fetch_update(
            // Only use `Relaxed` because this atomic carries no data dependencies.
            atomic::Ordering::Relaxed,
            atomic::Ordering::Relaxed,
            |len| {
                if len == self.capacity() {
                    None
                } else {
                    Some(len + 1)
                }
            },
        ) {
            Ok(old_len) => old_len,
            Err(_) => return Err(value),
        };

        // SAFETY: We never read from this data type without exclusive access.
        let val = unsafe { &mut *self.data[old_len].get() };
        *val = MaybeUninit::new(value);
        Ok(unsafe { &mut *val.as_mut_ptr() })
    }

    #[cfg(test)]
    fn iter_maybe_uninit_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut MaybeUninit<T>> + DoubleEndedIterator + '_ {
        self.data[..*self.len.get_mut()]
            .iter_mut()
            .map(UnsafeCell::get_mut)
    }
    #[cfg(test)]
    pub(crate) unsafe fn iter_assume_init_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut T> + DoubleEndedIterator + '_ {
        self.iter_maybe_uninit_mut()
            .map(|val| unsafe { &mut *val.as_mut_ptr() })
    }
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = T> + DoubleEndedIterator + '_ {
        let old_len = *self.len.get_mut();
        *self.len.get_mut() = 0;

        self.data[..old_len].iter_mut().map(|cell| {
            let value = mem::replace(cell.get_mut(), MaybeUninit::uninit());
            unsafe { value.assume_init() }
        })
    }
    pub(crate) fn into_iter(mut self) -> impl Iterator<Item = T> + DoubleEndedIterator {
        let data = mem::replace(&mut self.data, Vec::new().into_boxed_slice());
        let len = *self.len.get_mut();
        mem::forget(self);

        Vec::from(data).into_iter().take(len).map(|cell| {
            let value = cell.into_inner();
            unsafe { value.assume_init() }
        })
    }

    pub(crate) fn clear(&mut self) {
        self.drain().for_each(drop);
    }
}

impl<T> Debug for ConcurrentSlice<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConcurrentSlice")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .finish()
    }
}

impl<T> Drop for ConcurrentSlice<T> {
    fn drop(&mut self) {
        self.clear();
    }
}

unsafe impl<T: Send> Send for ConcurrentSlice<T> {}
unsafe impl<T: Send + Sync> Sync for ConcurrentSlice<T> {}

#[cfg(test)]
mod tests {
    use crate::concurrent_slice::ConcurrentSlice;
    use crate::test_util::assert_thread_safe;

    #[test]
    fn empty() {
        let mut slice = ConcurrentSlice::new(0);

        assert_eq!(slice.capacity(), 0);
        assert_eq!(slice.len(), 0);
        assert_eq!(slice.push(1), Err(1));
        slice.clear();
    }

    #[test]
    fn push() {
        let mut slice = ConcurrentSlice::new(3);
        assert_eq!(slice.capacity(), 3);

        assert_eq!(slice.push("1".to_owned()).unwrap(), "1");
        assert_eq!(slice.push("2".to_owned()).unwrap(), "2");
        assert_eq!(slice.push("3".to_owned()).unwrap(), "3");
        assert_eq!(slice.push("4".to_owned()), Err("4".to_owned()));

        assert_eq!(
            unsafe { slice.iter_assume_init_mut() }
                .map(|x| &**x)
                .collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
        assert_eq!(slice.drain().collect::<Vec<_>>(), ["1", "2", "3"]);

        let v1 = slice.push("1".to_owned()).unwrap();
        let v2 = slice.push("2".to_owned()).unwrap();
        let v3 = slice.push("3".to_owned()).unwrap();
        assert_eq!(slice.push(String::new()), Err(String::new()));

        v1.push('x');
        v2.push('y');
        v3.push('z');

        assert_eq!(slice.into_iter().collect::<Vec<_>>(), ["1x", "2y", "3z"]);
    }

    #[test]
    fn thread_safe() {
        assert_thread_safe::<ConcurrentSlice<()>>();
    }
}
