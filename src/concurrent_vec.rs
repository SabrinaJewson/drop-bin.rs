
use crate::{ConcurrentList, ConcurrentSlice};

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
    pub(crate) fn push(&self, value: T) -> &mut T {
        match self.data.head() {
            Some(head) => Ok((head, value)),
            None => Err(value),
        }
            .and_then(|(head, value)| head.push(value))
            .unwrap_or_else(|value| {
                let slice = ConcurrentSlice::new(self.data.head().map_or(4, |head| {
                    let capacity = head.capacity();
                    capacity.checked_mul(2).unwrap_or(capacity)
                }));
                match self.data.push(slice).push(value) {
                    Ok(value) => value,
                    Err(_) => unreachable!(),
                }
            })
    }

    #[cfg(test)]
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.data.iter_mut().flat_map(|slice| slice.iter_mut().rev())
    }

    pub(crate) fn into_iter(self) -> impl Iterator<Item = T> {
        self.data.into_iter().flat_map(|slice| slice.into_iter().rev())
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
        value.push_str("x");
    }

    let required = ["4x", "3x", "2x", "1x", "0x"];

    assert_eq!(vec.iter_mut().map(|v| &**v).collect::<Vec<_>>(), required);
    assert_eq!(vec.into_iter().collect::<Vec<_>>(), required);
}

#[test]
fn test_thread_safe() {
    crate::assert_thread_safe::<ConcurrentVec<()>>();
}
