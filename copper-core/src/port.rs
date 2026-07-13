use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct DirtyHandle(Arc<AtomicBool>);
impl DirtyHandle {
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

// Non-Clone - one writer per wire, enforced by the compiler
pub struct Out<T, D = ()> {
    inner: Arc<Mutex<T>>,
    dirty: Arc<AtomicBool>,
    _domain: PhantomData<D>
} 

impl<T: PartialEq + Clone, D> Out<T, D> {
    pub fn write(&self, value: T) {
        let mut g = self.inner.lock().unwrap();
        if *g != value {
            *g = value;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn dirty_handle(&self) -> DirtyHandle {
        DirtyHandle(self.dirty.clone())
    }
}

// Clone - multiple readers allowed
pub struct In<T, D = ()> {
    inner: Arc<Mutex<T>>,
    _domain: PhantomData<D>
}

impl<T, D> Clone for In<T, D> {
    fn clone(&self) -> Self {
        In { inner: self.inner.clone(), _domain: PhantomData }
    }
}

impl<T: Clone, D> In<T, D> {
    pub fn read(&self) -> T {
        self.inner.lock().unwrap().clone()
    }
}

pub fn wire<T: Clone + PartialEq, D>(initial: T) -> (Out<T, D>, In<T, D>) {
    let inner = Arc::new(Mutex::new(initial));
    let dirty = Arc::new(AtomicBool::new(false));
    let out = Out {
        inner: inner.clone(),
        dirty: dirty.clone(),
        _domain: PhantomData,
    };
    let input = In {
        inner,
        _domain: PhantomData,
    };
    (out, input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        let (out, in_): (Out<u8, ()>, In<u8, ()>) = wire(0);
        out.write(42);
        assert_eq!(in_.read(), 42);
    }

    #[test]
    fn initial_value_readable_before_write() {
        let (_out, in_): (Out<u32, ()>, In<u32, ()>) = wire(99);
        assert_eq!(in_.read(), 99);
    }

    #[test]
    fn cloned_readers_all_see_updated_value() {
        let (out, in_a): (Out<u8, ()>, In<u8, ()>) = wire(0);
        let in_b = in_a.clone();
        let in_c = in_a.clone();
        out.write(7);
        assert_eq!(in_a.read(), 7);
        assert_eq!(in_b.read(), 7);
        assert_eq!(in_c.read(), 7);
    }

    #[test]
    fn dirty_not_set_before_any_write() {
        let (out, _in): (Out<u8, ()>, In<u8, ()>) = wire(0);
        let handle = out.dirty_handle();
        assert!(!handle.take());
    }

    #[test]
    fn dirty_set_after_write_with_new_value() {
        let (out, _in): (Out<u8, ()>, In<u8, ()>) = wire(0);
        let handle = out.dirty_handle();
        out.write(1);
        assert!(handle.take());
    }

    #[test]
    fn dirty_cleared_after_take() {
        let (out, _in): (Out<u8, ()>, In<u8, ()>) = wire(0);
        let handle = out.dirty_handle();
        out.write(1);
        handle.take(); // clears
        assert!(!handle.take());
    }

    #[test]
    fn dirty_not_set_when_value_unchanged() {
        let (out, _in): (Out<u8, ()>, In<u8, ()>) = wire(5);
        let handle = out.dirty_handle();
        out.write(5); // same value — should not mark dirty
        assert!(!handle.take());
    }

    #[test]
    fn dirty_set_again_after_second_distinct_write() {
        let (out, _in): (Out<u8, ()>, In<u8, ()>) = wire(0);
        let handle = out.dirty_handle();
        out.write(1);
        handle.take(); // clears
        out.write(2);
        assert!(handle.take());
    }

    #[test]
    fn multiple_dirty_handles_share_same_flag() {
        let (out, _in): (Out<u8, ()>, In<u8, ()>) = wire(0);
        let h1 = out.dirty_handle();
        let h2 = out.dirty_handle();
        out.write(1);
        // Both see the flag set; first take clears it, second sees false
        assert!(h1.take());
        assert!(!h2.take());
    }
}
