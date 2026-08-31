use nonmax::NonMaxU32;
use starforge_macro::Deref;

pub struct EntityRegistry {
    // arche_reg: ArchetypeRegistry,
    // sparse_reg: SparseSetRegistry,
}

/// A stable, generational reference to an entity in an [`EntityRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityKey {
    /// Slot index into the owning registry's internal storage.
    pub index: EntityIndex,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: EntityGeneration,
}

/// Non-`u32::MAX` slot index for entities.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct EntityIndex(NonMaxU32);

/// Non-`u32::MAX` generation token attached to an [`EntityKey`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct EntityGeneration(NonMaxU32);

impl EntityIndex {
    /// Creates an index from a raw `u32`, rejecting `u32::MAX`.
    pub fn new(value: u32) -> Option<Self> {
        NonMaxU32::new(value).map(Self)
    }

    /// Creates an index from `usize`, rejecting values larger than `u32::MAX - 1`.
    pub fn from_usize(value: usize) -> Option<Self> {
        let value = u32::try_from(value).ok()?;
        Self::new(value)
    }
}

impl EntityGeneration {
    /// Creates a generation from a raw `u32`, rejecting `u32::MAX`.
    pub fn new(value: u32) -> Option<Self> {
        NonMaxU32::new(value).map(Self)
    }

    /// Advances to the next generation, wrapping from `u32::MAX - 1` to `0`.
    pub fn next(self) -> Self {
        if self.get() == u32::MAX - 1 {
            // `0` is always representable because only `u32::MAX` is rejected by `NonMaxU32`.
            Self::new(0).unwrap()
        } else {
            Self::new(self.get() + 1).expect("next generation must be representable")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn option_entity_key_has_same_size_as_entity_key() {
        assert_eq!(size_of::<EntityKey>(), size_of::<Option<EntityKey>>());
    }
}
