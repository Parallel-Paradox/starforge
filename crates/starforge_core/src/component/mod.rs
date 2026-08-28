mod meta;
mod registry;

use std::{
    alloc::{Layout, alloc, dealloc, handle_alloc_error},
    ptr::NonNull,
};

use crate::prelude::*;

pub use meta::{ComponentKind, ComponentMeta};
pub use registry::{ComponentGeneration, ComponentIndex, ComponentKey, ComponentRegistry, Error};

/// Marker trait for component types. Use `#[derive(Component)]` as convenience access.
pub trait Component: 'static + Send + Sync {}

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
    ///
    /// Zero-sized components skip the allocation and land on a dangling aligned
    /// pointer; their drop glue still runs when the parcel is dropped.
    /// Allocation failure aborts the process.
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

    /// Borrows the stored component as `T`.
    ///
    /// # Safety
    ///
    /// `T` must be the exact component type this parcel was created with.
    /// A mismatch panics in debug builds, but is undefined behavior in release builds.
    /// Prefer [`ComponentParcel::try_get`] when `T` is not statically known.
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
    /// A mismatch panics in debug builds, but is undefined behavior in release builds.
    /// Prefer [`ComponentParcel::try_get_mut`] when `T` is not statically known.
    pub unsafe fn get_mut<T: Component>(&mut self) -> &mut T {
        debug_assert_eq!(self.type_id, TypeId::of::<T>(), "ComponentParcel type mismatch");
        // SAFETY: parcel owns `buf_ptr`, which holds a live `T` written in `new`.
        unsafe { &mut *self.buf_ptr.as_ptr().cast::<T>() }
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Component, Debug, PartialEq)]
    struct Pos {
        x: i32,
        y: i32,
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
    fn non_trivial_component_is_dropped_by_parcel() {
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Boxed(#[allow(dead_code)] Box<u8>);
        impl Drop for Boxed {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        impl Component for Boxed {}

        let parcel = ComponentParcel::new(Boxed(Box::new(1)));
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
        drop(parcel);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn zst_with_drop_drops_at_parcel_drop() {
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
}
