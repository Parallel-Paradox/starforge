use crate::prelude::*;
use std::any::Any;

/// Metadata describing a registered type: its identity, size, alignment, and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeMeta {
    pub id: TypeId,
    pub size: usize,
    pub align: usize,
    pub name: TypeName,
}

impl TypeMeta {
    /// Builds `TypeMeta` for a native Rust type `T` from `size_of`/`align_of`/`type_name`.
    pub fn of<T: Any + 'static>() -> Self {
        Self::new(
            TypeId::of::<T>(),
            std::mem::size_of::<T>(),
            std::mem::align_of::<T>(),
            TypeName::of::<T>(),
        )
    }

    /// Constructs `TypeMeta` from explicit values. Zero-sized types (size 0) are allowed;
    /// `size`/`align` invariants are otherwise checked via `debug_assert!` in debug builds.
    pub fn new(id: TypeId, size: usize, align: usize, name: TypeName) -> Self {
        // zero-sized types (e.g. marker structs) are allowed and have size 0
        debug_assert!(size == 0 || size >= align, "Type size must be ge to alignment.");
        debug_assert!(size % align == 0, "Type size must be a multiple of alignment.");
        debug_assert!(align.is_power_of_two(), "Type alignment must be a power of two.");
        Self { id, size, align, name }
    }
}
