use crate::ConcurrentList;
use crate::ConcurrentSlice;

/// A concurrent append-only vector built from a `ConcurrentList<ConcurrentSlice<T>>`.
#[derive(Debug)]
pub struct ConcurrentVec<T> {
    data: ConcurrentList<ConcurrentSlice<T>>,
}

impl<T> ConcurrentVec<T> {
    pub(crate) const fn new() -> Self {
        Self {
            data: ConcurrentList::new(),
        }
    }

    // This is safe because this container cannot be immutably iterated over
    #[allow(clippy::mut_from_ref)]
    pub(crate) fn push(&self, mut value: T) -> &mut T {
        loop {
            if let Some(head) = self.data.head() {
                match head.push(value) {
                    Ok(r) => break r,
                    Err(value_returned) => value = value_returned,
                }
            }

            let slice = ConcurrentSlice::new(self.data.head().map_or(4, |head| {
                let capacity = head.capacity();
                capacity.checked_mul(2).unwrap_or(capacity)
            }));
            self.data.push(slice);
        }
    }

    #[cfg(test)]
    pub(crate) unsafe fn iter_assume_init_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.data
            .iter_mut()
            .flat_map(|slice| unsafe { slice.iter_assume_init_mut() }.rev())
    }

    pub(crate) fn into_iter(self) -> impl Iterator<Item = T> {
        self.data
            .into_iter()
            .flat_map(|slice| slice.into_iter().rev())
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.data.iter().map(ConcurrentSlice::len).sum()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl<T> Default for ConcurrentVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::concurrent_vec::ConcurrentVec;
    use crate::test_util::assert_thread_safe;

    #[test]
    fn test() {
        let mut vec = ConcurrentVec::new();
        assert_eq!(vec.len(), 0);
        assert!(vec.is_empty());

        let mut values = (0..5)
            .map(|n| {
                assert_eq!(vec.len(), n);
                let r = vec.push(n.to_string());
                assert_eq!(vec.len(), n + 1);
                assert!(!vec.is_empty());
                r
            })
            .collect::<Vec<_>>();

        for value in &mut values {
            value.push('x');
        }

        let required = ["4x", "3x", "2x", "1x", "0x"];

        assert_eq!(
            unsafe { vec.iter_assume_init_mut() }
                .map(|v| &**v)
                .collect::<Vec<_>>(),
            required
        );
        assert_eq!(vec.into_iter().collect::<Vec<_>>(), required);
    }

    #[test]
    fn thread_safe() {
        assert_thread_safe::<ConcurrentVec<()>>();
    }
}
