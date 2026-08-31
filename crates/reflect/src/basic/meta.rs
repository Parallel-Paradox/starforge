use std::{alloc::Layout, any::Any};

use crate::prelude::*;

/// Contains basic metadata about a type. The minimum information needed to erase a type
/// while still be able to identify it and safely own and drop its instance.
#[derive(Debug, Clone)]
pub struct TypeMeta {
    id: TypeId,
    name: TypeName,
    needs_drop: NeedsDrop,
    layout: Layout,
}

impl PartialEq for TypeMeta {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeMeta {}

/// Indicates whether a type requires a drop implementation to be called.
#[derive(Debug, Clone, Copy)]
pub enum NeedsDrop {
    /// Data-only types.
    Trivial,
    /// Types that [`std::mem::needs_drop`] returns true.
    NonTrivial { drop_fn: unsafe fn(*mut u8) },
}

impl TypeMeta {
    /// The unique identifier of the type.
    pub fn id(&self) -> TypeId {
        self.id
    }

    /// The name of the type, useful for debugging and logging purposes.
    pub fn name(&self) -> &TypeName {
        &self.name
    }

    /// Indicates whether the type requires a drop implementation to be called.
    pub fn needs_drop(&self) -> &NeedsDrop {
        &self.needs_drop
    }

    /// The memory layout of the type, including size and alignment.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Creates a new `TypeMeta` instance for the given type `T`.
    pub fn new<T: Any>() -> Self {
        let id = TypeId::of::<T>();
        let name = TypeName::of::<T>();
        let needs_drop = if std::mem::needs_drop::<T>() {
            NeedsDrop::NonTrivial {
                drop_fn: |ptr| unsafe { std::ptr::drop_in_place(ptr as *mut T) },
            }
        } else {
            NeedsDrop::Trivial
        };
        let layout = Layout::new::<T>();

        Self { id, name, needs_drop, layout }
    }

    /// Creates a new `TypeMeta` instance with the provided parameters.
    /// Useful for creating metadata that may not be known at compile time such as scripting.
    pub fn new_impl(id: TypeId, name: TypeName, needs_drop: NeedsDrop, layout: Layout) -> Self {
        Self { id, name, needs_drop, layout }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_derives_metadata_from_type() {
        let meta = TypeMeta::new::<u64>();

        assert_eq!(meta.id(), TypeId::of::<u64>());
        assert_eq!(meta.name(), &TypeName::of::<u64>());
        assert!(matches!(meta.needs_drop(), NeedsDrop::Trivial));
        assert_eq!(meta.layout(), Layout::new::<u64>());
    }

    #[test]
    fn new_marks_droppable_types_as_non_trivial() {
        let meta = TypeMeta::new::<String>();
        assert!(matches!(meta.needs_drop(), NeedsDrop::NonTrivial { .. }));
    }

    #[test]
    fn equality_ignores_derived_attributes() {
        let type_id = TypeId::of_script(1);
        let a = TypeMeta::new_impl(
            type_id,
            TypeName::of_script("v1"),
            NeedsDrop::Trivial,
            Layout::new::<u8>(),
        );
        let b = TypeMeta::new_impl(
            type_id,
            TypeName::of_script("v2"),
            NeedsDrop::NonTrivial {
                drop_fn: |ptr| unsafe { std::ptr::drop_in_place(ptr as *mut String) },
            },
            Layout::new::<String>(),
        );

        // identity is defined by `id` alone; name/drop/layout are derived attributes of it
        assert_eq!(a, b);
    }
}
