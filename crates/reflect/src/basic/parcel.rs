use std::{
    alloc::{alloc, dealloc, handle_alloc_error},
    any::Any,
    ptr::NonNull,
};

use crate::{basic::meta::NeedsDrop, prelude::*};

pub struct Parcel {
    meta: TypeMeta,
    data: NonNull<u8>,
}

impl Parcel {
    pub fn new<T: Any>(value: T) -> Self {
        let meta = TypeMeta::new::<T>();

        let layout = meta.layout();
        // ZSTs skip allocation.
        let data = if layout.size() == 0 {
            NonNull::<u8>::dangling()
        } else {
            // SAFETY: non-zero size; OOM aborts via `handle_alloc_error`.
            NonNull::new(unsafe { alloc(layout) }).unwrap_or_else(|| handle_alloc_error(layout))
        };
        // SAFETY: `buf_ptr` is aligned to `T`, and it is either sized for `T`
        // or a dangling pointer valid for a zero-sized write.
        unsafe { data.as_ptr().cast::<T>().write(value) };

        Self { meta, data }
    }
}

impl Drop for Parcel {
    fn drop(&mut self) {
        if let NeedsDrop::NonTrivial { drop_fn } = self.meta().needs_drop() {
            // SAFETY: `data` always holds an instance of `T` written in `new`.
            unsafe { drop_fn(self.data.as_ptr()) };
        }

        if self.meta().layout().size() > 0 {
            // SAFETY: `data` was allocated with the same layout in `new`.
            unsafe { dealloc(self.data.as_ptr(), self.meta().layout()) };
        }
    }
}

impl Default for Parcel {
    fn default() -> Self {
        Self::new(())
    }
}

impl Parcel {
    pub fn meta(&self) -> &TypeMeta {
        &self.meta
    }

    /// Takes the stored value out of the parcel, moving it out of the buffer
    /// and deallocating the buffer without running the value's drop glue.
    ///
    /// # Safety
    ///
    /// `T` must be the exact type this parcel was created with.
    pub unsafe fn take<T: Any>(self) -> T {
        debug_assert_eq!(self.meta.id(), TypeId::of::<T>(), "Parcel type mismatch");

        // Prevent the parcel's `Drop` from destroying `T` after it is moved out.
        let parcel = std::mem::ManuallyDrop::new(self);

        // SAFETY: caller guarantees `data` holds a live `T`; `read` copies it
        // out, leaving the buffer to be reclaimed without drop glue.
        let value = unsafe { parcel.data.as_ptr().cast::<T>().read() };

        // SAFETY: `data` is only the product of `alloc` when size > 0;
        // zero-sized parcels never allocate, so nothing needs deallocating.
        if parcel.meta.layout().size() > 0 {
            unsafe { dealloc(parcel.data.as_ptr(), parcel.meta.layout()) };
        }

        value
    }

    /// Borrows the stored value as `T`.
    ///
    /// # Safety
    ///
    /// `T` must be the exact type this parcel was created with.
    pub unsafe fn get<T: Any>(&self) -> &T {
        debug_assert_eq!(self.meta.id(), TypeId::of::<T>(), "Parcel type mismatch");
        // SAFETY: parcel owns `data`, which holds a live `T` written in `new`.
        unsafe { &*self.data.as_ptr().cast::<T>() }
    }

    /// Mutably borrows the stored value as `T`.
    ///
    /// # Safety
    ///
    /// `T` must be the exact type this parcel was created with.
    pub unsafe fn get_mut<T: Any>(&mut self) -> &mut T {
        debug_assert_eq!(self.meta.id(), TypeId::of::<T>(), "Parcel type mismatch");
        // SAFETY: parcel owns `data`, which holds a live `T` written in `new`.
        unsafe { &mut *self.data.as_ptr().cast::<T>() }
    }

    /// Takes the stored value out of the parcel as `T`, returning `None` when
    /// `T` does not match the parcel's type.
    ///
    /// On success the parcel is left empty, owning nothing. On failure the
    /// parcel is left untouched and remains fully usable.
    pub fn try_take<T: Any>(&mut self) -> Option<T> {
        if self.meta.id() != TypeId::of::<T>() {
            // Failure: `self` still owns the value, untouched.
            return None;
        }

        let parcel = std::mem::replace(self, Self::default());
        // SAFETY: the type check above guarantees `T` matches the parcel.
        Some(unsafe { parcel.take::<T>() })
    }

    /// Borrows the stored value as `T`, returning `None` when `T` does not
    /// match the parcel's type.
    pub fn try_get<T: Any>(&self) -> Option<&T> {
        if self.meta.id() == TypeId::of::<T>() {
            unsafe { Some(self.get::<T>()) }
        } else {
            None
        }
    }

    /// Mutably borrows the stored value as `T`, returning `None` when `T` does
    /// not match the parcel's type.
    pub fn try_get_mut<T: Any>(&mut self) -> Option<&mut T> {
        if self.meta.id() == TypeId::of::<T>() {
            unsafe { Some(self.get_mut::<T>()) }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[derive(Debug, PartialEq)]
    struct Pos {
        x: i32,
        y: i32,
    }

    /// Test-local drop counter. Each test owns its own `Arc<Tracker>`, so tests
    /// never share mutable state and can run in parallel.
    #[derive(Default)]
    struct Tracker {
        count: AtomicUsize,
    }

    impl Tracker {
        fn new() -> Arc<Self> {
            Arc::default()
        }
    }

    /// A non-trivial value; dropping it records the drop on the [`Tracker`]
    /// it holds a handle to.
    struct Tracked {
        tracker: Arc<Tracker>,
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.tracker.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn value_is_written_into_buffer() {
        let parcel = Parcel::new(Pos { x: 1, y: 2 });
        let pos = unsafe { parcel.get::<Pos>() };
        assert_eq!(*pos, Pos { x: 1, y: 2 });
        assert_eq!(parcel.meta().layout().size(), size_of::<Pos>());
        assert!(matches!(parcel.meta().needs_drop(), NeedsDrop::Trivial));
    }

    #[test]
    fn zst_with_drop_drops_at_parcel_drop() {
        // A ZST cannot carry the `Arc<Tracker>` used elsewhere, so this test
        // tracks drops through its own static counter.
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct ZstDrop;
        impl Drop for ZstDrop {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }

        let parcel = Parcel::new(ZstDrop);
        // ZST parcels skip allocation but still drop their value at parcel drop.
        assert_eq!(parcel.meta().layout().size(), 0);
        assert!(matches!(parcel.meta().needs_drop(), NeedsDrop::NonTrivial { .. }));
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
        drop(parcel);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn take_returns_owned_value() {
        let parcel = Parcel::new(Pos { x: 3, y: 4 });
        let pos = unsafe { parcel.take::<Pos>() };
        assert_eq!(pos, Pos { x: 3, y: 4 });
    }

    #[test]
    fn take_moves_value_out_without_double_drop() {
        let tracker = Tracker::new();
        let parcel = Parcel::new(Tracked { tracker: tracker.clone() });
        // The returned value is dropped at the end of the statement; the parcel
        // must not run `Tracked`'s glue a second time.
        unsafe { parcel.take::<Tracked>() };
        assert_eq!(tracker.count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn try_take_matching_type_leaves_parcel_empty() {
        let mut parcel = Parcel::new(Pos { x: 5, y: 6 });
        let pos = parcel.try_take::<Pos>();
        assert_eq!(pos, Some(Pos { x: 5, y: 6 }));
        // The parcel survives but now owns nothing.
        assert_eq!(parcel.meta().id(), TypeId::of::<()>());
        assert_eq!(parcel.meta().layout().size(), 0);
        assert!(parcel.try_get::<Pos>().is_none());
    }

    #[test]
    fn try_take_mismatched_type_leaves_parcel_untouched() {
        let tracker = Tracker::new();
        let mut parcel = Parcel::new(Tracked { tracker: tracker.clone() });
        let taken: Option<Pos> = parcel.try_take();
        assert_eq!(taken, None);
        assert_eq!(tracker.count.load(Ordering::SeqCst), 0); // self is not consumed
        // The parcel still owns its value and remains usable.
        assert!(parcel.try_get::<Tracked>().is_some());
        drop(parcel);
        assert_eq!(tracker.count.load(Ordering::SeqCst), 1);
    }
}
