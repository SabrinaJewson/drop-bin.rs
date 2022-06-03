use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::ptr;
use std::sync::atomic;
use std::sync::atomic::AtomicPtr;

/// A concurrent insert-only linked list.
pub(crate) struct ConcurrentList<T> {
    head: AtomicPtr<Node<T>>,
}

struct Node<T> {
    value: T,
    next: *mut Node<T>,
}

unsafe impl<T: Send> Send for Node<T> {}
unsafe impl<T: Send + Sync> Sync for Node<T> {}

impl<T> ConcurrentList<T> {
    pub(crate) const fn new() -> Self {
        Self {
            head: AtomicPtr::new(ptr::null_mut()),
        }
    }

    fn head_node(&self) -> Option<&Node<T>> {
        let head = self.head.load(atomic::Ordering::Relaxed);

        if head.is_null() {
            None
        } else {
            Some(unsafe { &*head })
        }
    }
    #[cfg(test)]
    fn head_node_mut(&mut self) -> Option<&mut Node<T>> {
        let head = *self.head.get_mut();

        if head.is_null() {
            None
        } else {
            Some(unsafe { &mut *head })
        }
    }

    pub(crate) fn head(&self) -> Option<&T> {
        self.head_node().map(|node| &node.value)
    }
    #[cfg(test)]
    pub(crate) fn head_mut(&mut self) -> Option<&mut T> {
        self.head_node_mut().map(|node| &mut node.value)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        let mut node = self.head.load(atomic::Ordering::Relaxed);

        std::iter::from_fn(move || {
            if node.is_null() {
                None
            } else {
                let this_node = unsafe { &*node };
                node = this_node.next;
                Some(&this_node.value)
            }
        })
    }
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        let mut node = *self.head.get_mut();

        std::iter::from_fn(move || {
            if node.is_null() {
                None
            } else {
                let this_node = unsafe { &mut *node };
                node = this_node.next;
                Some(&mut this_node.value)
            }
        })
    }
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(move || self.pop())
    }
    pub(crate) fn into_iter(mut self) -> impl Iterator<Item = T> {
        std::iter::from_fn(move || self.pop())
    }

    pub(crate) fn push(&self, value: T) -> &T {
        let node = Box::into_raw(Box::new(Node {
            value,
            // Any value
            next: ptr::null_mut(),
        }));

        let mut head = self.head.load(atomic::Ordering::Relaxed);

        loop {
            unsafe { &mut *node }.next = head;

            match self.head.compare_exchange_weak(
                head,
                node,
                atomic::Ordering::Release,
                atomic::Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(updated_head) => head = updated_head,
            }
        }

        &unsafe { &*node }.value
    }
    pub(crate) fn pop(&mut self) -> Option<T> {
        let head_ptr = self.head.get_mut();
        if head_ptr.is_null() {
            None
        } else {
            let head_node = unsafe { Box::from_raw(*head_ptr) };
            *head_ptr = head_node.next;
            Some(head_node.value)
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.iter().count()
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.head.load(atomic::Ordering::Relaxed).is_null()
    }
}

impl<T> Default for ConcurrentList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Debug> Debug for ConcurrentList<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T> Drop for ConcurrentList<T> {
    fn drop(&mut self) {
        self.drain().for_each(drop);
    }
}

#[cfg(test)]
mod tests {
    use crate::concurrent_list::ConcurrentList;
    use crate::test_util::assert_thread_safe;
    use std::ptr;

    #[test]
    fn null() {
        let mut list: ConcurrentList<()> = ConcurrentList::new();
        assert_eq!(*list.head.get_mut(), ptr::null_mut());
        assert_eq!(list.head(), None);
        assert_eq!(list.head_mut(), None);
        assert_eq!(list.iter().next(), None);
        assert_eq!(list.iter_mut().next(), None);
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
    }

    #[test]
    fn push() {
        let list = ConcurrentList::new();

        let r = list.push("Hello World".to_owned());
        assert_eq!(r, "Hello World");

        assert_eq!(list.head().unwrap() as *const String, r as *const String);
        assert_eq!(
            list.iter().map(|x| x as *const String).collect::<Vec<_>>(),
            [r as *const String]
        );

        let r2 = list.push("Foo".to_owned());
        assert_eq!(r, "Hello World");
        assert_eq!(r2, "Foo");

        assert_eq!(list.head().unwrap() as *const String, r2 as *const String);
        assert_eq!(
            list.iter().map(|x| x as *const String).collect::<Vec<_>>(),
            [r2 as *const String, r as *const String]
        );

        assert_eq!(list.into_iter().collect::<Vec<_>>(), ["Foo", "Hello World"]);
    }

    #[test]
    fn pop() {
        let mut list = ConcurrentList::new();

        list.push("1".to_owned());
        list.push("2".to_owned());
        list.push("3".to_owned());

        assert_eq!(list.pop().unwrap(), "3");
        assert_eq!(list.pop().unwrap(), "2");
        assert_eq!(list.pop().unwrap(), "1");
        assert_eq!(list.pop(), None);

        list.push("1".to_owned());
        list.push("2".to_owned());
        list.push("3".to_owned());

        let mut iter = list.into_iter();
        assert_eq!(iter.next().unwrap(), "3");
        drop(iter);
    }

    #[test]
    fn thread_safe() {
        assert_thread_safe::<ConcurrentList<()>>();
    }
}
