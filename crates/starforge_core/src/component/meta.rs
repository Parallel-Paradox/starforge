use crate::prelude::*;

/// How a component's storage should be dropped.
#[derive(Debug, Clone, Copy)]
pub enum ComponentKind {
    /// No drop glue is needed; the storage can simply be forgotten.
    Trivial,
    /// Requires calling `drop_fn` on the raw storage to release it.
    NonTrivial { drop_fn: unsafe fn(*mut u8) },
}

impl ComponentKind {
    pub fn of<T: Component>() -> Self {
        if std::mem::needs_drop::<T>() {
            ComponentKind::NonTrivial { drop_fn: drop_in_place_erased::<T> }
        } else {
            ComponentKind::Trivial
        }
    }
}

/// Metadata describing a registered component: its underlying type and drop behavior.
#[derive(Debug, Clone)]
pub struct ComponentMeta {
    pub id: TypeId,
    pub kind: ComponentKind,
}

impl PartialEq for ComponentMeta {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ComponentMeta {}

impl ComponentMeta {
    /// Builds `ComponentMeta` for component type `T`, deriving drop behavior from `Drop`.
    pub fn of<T: Component>() -> Self {
        Self::new(TypeId::of::<T>(), ComponentKind::of::<T>())
    }

    /// Constructs `ComponentMeta` from explicit values.
    pub fn new(id: TypeId, kind: ComponentKind) -> Self {
        Self { id, kind }
    }
}

/// Type-erased drop glue for `T`, used to destroy components stored as raw bytes.
pub unsafe fn drop_in_place_erased<T>(ptr: *mut u8) {
    // SAFETY: caller guarantees `ptr` points at a valid, initialized `T`
    unsafe { std::ptr::drop_in_place(ptr as *mut T) };
}
