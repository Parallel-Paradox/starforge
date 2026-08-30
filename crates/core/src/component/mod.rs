mod meta;
mod registry;

use std::{
    alloc::{Layout, alloc, dealloc, handle_alloc_error},
    any::Any,
    ptr::NonNull,
};

use crate::prelude::*;

pub use meta::{ComponentKind, ComponentMeta};
pub use registry::{ComponentGeneration, ComponentIndex, ComponentKey, ComponentRegistry, Error};

/// Marker trait for component types. Use `#[derive(Component)]` as convenience access.
pub trait Component: 'static + Any + Send + Sync {}

/// Type-erased ownership of a single component value.
///
/// A `ComponentParcel` owns one value of component type `T` stored in a heap
/// buffer. The value is dropped when the parcel is dropped.
pub struct ComponentParcel {
    name: TypeName,
    type_id: TypeId,
    kind: ComponentKind,
    layout: Layout,
    buf_ptr: NonNull<u8>,
}

impl ComponentParcel {
    /// Returns the [`TypeName`] of the stored component type.
    pub fn name(&self) -> &TypeName {
        &self.name
    }

    /// Returns the [`TypeId`] of the stored component type.
    pub fn type_id(&self) -> &TypeId {
        &self.type_id
    }

    /// Creates a parcel owning `component`, allocating a buffer sized for `T`.
    pub fn new<T: Component>(component: T) -> Self {
        let layout = Layout::new::<T>();

        // ZSTs skip allocation.
        let buf_ptr = if layout.size() == 0 {
            NonNull::<u8>::dangling()
        } else {
            // SAFETY: non-zero size; OOM aborts via `handle_alloc_error`.
            NonNull::new(unsafe { alloc(layout) }).unwrap_or_else(|| handle_alloc_error(layout))
        };

        // SAFETY: `buf_ptr` is aligned to `T`, and it is either sized for `T`
        // or a dangling pointer valid for a zero-sized write.
        unsafe { buf_ptr.as_ptr().cast::<T>().write(component) };

        Self {
            name: TypeName::of::<T>(),
            type_id: TypeId::of::<T>(),
            kind: ComponentKind::of::<T>(),
            layout,
            buf_ptr,
        }
    }

    /// Takes the stored component out of the parcel, moving it out of the
    /// buffer and deallocating the buffer without running the value's drop glue.
    ///
    /// # Safety
    ///
    /// `T` must be the exact component type this parcel was created with.
    pub unsafe fn take<T: Component>(self) -> T {
        debug_assert_eq!(self.type_id, TypeId::of::<T>(), "ComponentParcel type mismatch");

        // Prevent the parcel's `Drop` from destroying `T` after it is moved out.
        let parcel = std::mem::ManuallyDrop::new(self);

        // SAFETY: caller guarantees `buf_ptr` holds a live `T`; `read` copies it
        // out, leaving the buffer to be reclaimed without drop glue.
        let value = unsafe { parcel.buf_ptr.as_ptr().cast::<T>().read() };

        // SAFETY: `buf_ptr` is only the product of `alloc` when size > 0;
        // zero-sized parcels never allocate, so nothing needs deallocating.
        if parcel.layout.size() > 0 {
            unsafe { dealloc(parcel.buf_ptr.as_ptr(), parcel.layout) };
        }

        value
    }

    /// Borrows the stored component as `T`.
    ///
    /// # Safety
    ///
    /// `T` must be the exact component type this parcel was created with.
    pub unsafe fn get<T: Component>(&self) -> &T {
        debug_assert_eq!(self.type_id, TypeId::of::<T>(), "ComponentParcel type mismatch");
        // SAFETY: parcel owns `buf_ptr`, which holds a live `T` written in `new`.
        unsafe { &*self.buf_ptr.as_ptr().cast::<T>() }
    }

    /// Mutably borrows the stored component as `T`.
    ///
    /// # Safety
    ///
    /// `T` must be the exact component type this parcel was created with.
    pub unsafe fn get_mut<T: Component>(&mut self) -> &mut T {
        debug_assert_eq!(self.type_id, TypeId::of::<T>(), "ComponentParcel type mismatch");
        // SAFETY: parcel owns `buf_ptr`, which holds a live `T` written in `new`.
        unsafe { &mut *self.buf_ptr.as_ptr().cast::<T>() }
    }

    /// Takes the stored component out of the parcel as `T`, returning `None`
    /// when `T` does not match the parcel's component type.
    ///
    /// On success the parcel is left empty, owning nothing. On failure the
    /// parcel is left untouched and remains fully usable.
    pub fn try_take<T: Component>(&mut self) -> Option<T> {
        if self.type_id != TypeId::of::<T>() {
            // Failure: `self` still owns the value, untouched.
            return None;
        }

        let parcel = std::mem::replace(self, Self::default());
        // SAFETY: the type check above guarantees `T` matches the parcel.
        Some(unsafe { parcel.take::<T>() })
    }

    /// Borrows the stored component as `T`, returning `None` when `T` does not
    /// match the parcel's component type.
    pub fn try_get<T: Component>(&self) -> Option<&T> {
        if self.type_id == TypeId::of::<T>() {
            unsafe { Some(self.get::<T>()) }
        } else {
            None
        }
    }

    /// Mutably borrows the stored component as `T`, returning `None` when `T`
    /// does not match the parcel's component type.
    pub fn try_get_mut<T: Component>(&mut self) -> Option<&mut T> {
        if self.type_id == TypeId::of::<T>() {
            unsafe { Some(self.get_mut::<T>()) }
        } else {
            None
        }
    }
}

impl Default for ComponentParcel {
    /// A parcel that owns no value; dropping it is a no-op. Used as a
    /// placeholder when the real parcel has been taken out, e.g. by
    /// [`ComponentParcel::try_take`].
    fn default() -> Self {
        Self {
            name: TypeName::of::<()>(),
            type_id: TypeId::of::<()>(),
            kind: ComponentKind::Trivial,
            layout: Layout::new::<()>(),
            // `()` is a ZST, so a dangling pointer is a valid `*mut u8`.
            buf_ptr: NonNull::<u8>::dangling(),
        }
    }
}

impl Drop for ComponentParcel {
    fn drop(&mut self) {
        if let ComponentKind::NonTrivial { drop_fn } = self.kind {
            // SAFETY: `buf_ptr` always holds an initialized `T` (written in `new`).
            unsafe { drop_fn(self.buf_ptr.as_ptr()) };
        }

        // SAFETY: `buf_ptr` is only the product of `alloc` when size > 0;
        // zero-sized parcels never allocate, so nothing needs deallocating.
        if self.layout.size() > 0 {
            unsafe { dealloc(self.buf_ptr.as_ptr(), self.layout) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Component, Debug, PartialEq)]
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

    /// A non-trivial component; dropping it records the drop on the [`Tracker`]
    /// it holds a handle to.
    #[derive(Component)]
    struct Tracked {
        tracker: Arc<Tracker>,
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.tracker.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn component_is_written_into_buffer() {
        let parcel = ComponentParcel::new(Pos { x: 1, y: 2 });
        let pos = unsafe { parcel.get::<Pos>() };
        assert_eq!(*pos, Pos { x: 1, y: 2 });
        assert_eq!(parcel.layout.size(), size_of::<Pos>());
        assert!(matches!(parcel.kind, ComponentKind::Trivial));
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
        impl Component for ZstDrop {}

        let parcel = ComponentParcel::new(ZstDrop);
        // ZST parcels skip allocation but still drop their value at parcel drop.
        assert_eq!(parcel.layout.size(), 0);
        assert!(matches!(parcel.kind, ComponentKind::NonTrivial { .. }));
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
        drop(parcel);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn take_returns_owned_value() {
        let parcel = ComponentParcel::new(Pos { x: 3, y: 4 });
        let pos = unsafe { parcel.take::<Pos>() };
        assert_eq!(pos, Pos { x: 3, y: 4 });
    }

    #[test]
    fn take_moves_value_out_without_double_drop() {
        let tracker = Tracker::new();
        let parcel = ComponentParcel::new(Tracked { tracker: tracker.clone() });
        // The returned value is dropped at the end of the statement; the parcel
        // must not run `Tracked`'s glue a second time.
        unsafe { parcel.take::<Tracked>() };
        assert_eq!(tracker.count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn try_take_matching_type_leaves_parcel_empty() {
        let mut parcel = ComponentParcel::new(Pos { x: 5, y: 6 });
        let pos = parcel.try_take::<Pos>();
        assert_eq!(pos, Some(Pos { x: 5, y: 6 }));
        // The parcel survives but now owns nothing.
        assert_eq!(parcel.type_id, TypeId::of::<()>());
        assert_eq!(parcel.layout.size(), 0);
        assert!(parcel.try_get::<Pos>().is_none());
    }

    #[test]
    fn try_take_mismatched_type_leaves_parcel_untouched() {
        let tracker = Tracker::new();
        let mut parcel = ComponentParcel::new(Tracked { tracker: tracker.clone() });
        let taken: Option<Pos> = parcel.try_take();
        assert_eq!(taken, None);
        assert_eq!(tracker.count.load(Ordering::SeqCst), 0); // self is not consumed
        // The parcel still owns its value and remains usable.
        assert!(parcel.try_get::<Tracked>().is_some());
        drop(parcel);
        assert_eq!(tracker.count.load(Ordering::SeqCst), 1);
    }
}
