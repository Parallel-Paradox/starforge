mod registry;

use std::{
    alloc::{Layout, dealloc},
    ptr::NonNull,
    slice::{ChunksExact, ChunksExactMut},
};

use crate::{entity::EntityGeneration, prelude::EntityKey};
use starforge_macro::Deref;
use starforge_reflect::{basic::meta::NeedsDrop, prelude::TypeMeta};

pub use registry::{
    Error as RegistryError, SparseSetGeneration, SparseSetIndex, SparseSetKey, SparseSetRegistry,
};

use nonmax::NonMaxU32;
use thiserror::Error;

/// A sparse set storing one component type's dense data alongside an entity-to-dense
/// index, keyed by entity index.
pub struct SparseSet {
    meta: TypeMeta,
    /// Maps `entity_key.index` to the entity's dense row, `None` when the entity
    /// does not own a component in this set.
    entity_to_dense: Vec<Option<DenseIndex>>,
    entity_keys: Vec<EntityKey>,
    buf_ptr: NonNull<u8>,
    /// Number of elements `buf_ptr` was allocated to hold.
    capacity: usize,
}

/// Non-`u32::MAX` slot index into a [`SparseSet`]'s dense array.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct DenseIndex(NonMaxU32);

impl DenseIndex {
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

impl SparseSet {
    /// Metadata describing the stored component type.
    pub fn meta(&self) -> &TypeMeta {
        &self.meta
    }

    /// Returns a slice of the valid entities in the dense array.
    pub fn entity_keys(&self) -> &[EntityKey] {
        &self.entity_keys
    }

    /// Returns the number of entities currently stored in the dense array.
    pub fn len(&self) -> usize {
        self.entity_keys.len()
    }

    /// Returns `true` if the dense array currently stores no entities.
    pub fn is_empty(&self) -> bool {
        self.entity_keys.is_empty()
    }

    /// Builds an empty `SparseSet` for `meta`'s component type, without allocating a
    /// dense buffer.
    pub fn new(meta: TypeMeta) -> Self {
        Self {
            meta,
            entity_to_dense: Vec::new(),
            entity_keys: Vec::new(),
            buf_ptr: NonNull::dangling(),
            capacity: 0,
        }
    }

    /// Builds a `SparseSet` for `meta`'s component type with a dense buffer
    /// pre allocated to hold at least `capacity` elements, sized and aligned from the
    /// meta.
    pub fn with_capacity(meta: TypeMeta, capacity: usize) -> Self {
        let mut set = Self::new(meta);
        if capacity > 0 {
            set.reserve(capacity);
        }
        set
    }

    /// Ensures the dense buffer can hold at least `additional` more elements beyond
    /// `self.len()`, growing the allocation (by doubling, at minimum) if needed.
    pub fn reserve(&mut self, additional: usize) {
        let required = self.len().checked_add(additional).expect("reserve size overflow");
        if required <= self.capacity {
            return;
        }

        let new_capacity = required.max(self.capacity.saturating_mul(2)).max(4);

        let layout = self.meta.layout();
        let size = layout.size();
        let align = layout.align();
        if size != 0 {
            let new_layout = Layout::from_size_align(size * new_capacity, align)
                .expect("sparse set buffer layout must be valid");

            let new_ptr = if self.capacity == 0 {
                NonNull::new(unsafe { std::alloc::alloc(new_layout) })
                    .unwrap_or_else(|| std::alloc::handle_alloc_error(new_layout))
            } else {
                let old_layout = Layout::from_size_align(size * self.capacity, align)
                    .expect("sparse set buffer layout must be valid");
                // SAFETY: `buf_ptr` was allocated (or reallocated) with `old_layout`, and
                // `new_layout` shares its alignment with a strictly larger size.
                NonNull::new(unsafe {
                    std::alloc::realloc(self.buf_ptr.as_ptr(), old_layout, new_layout.size())
                })
                .unwrap_or_else(|| std::alloc::handle_alloc_error(new_layout))
            };

            self.buf_ptr = new_ptr;
        }

        self.capacity = new_capacity;
        self.entity_keys.reserve(additional);
    }

    /// Inserts `comp_data` as `entity_key`'s component in this set.
    ///
    /// When `entity_key` already owns a component in this set, drop the old value
    /// then use the new data to overwrite it.
    /// Grows the dense buffer (doubling capacity) if it is full.
    ///
    /// # Panics
    ///
    /// Panics if `comp_data` length does not match the meta's component size.
    pub fn insert(&mut self, entity_key: EntityKey, comp_data: &[u8]) {
        let size = self.meta.layout().size();
        assert_eq!(comp_data.len(), size, "component data length must match the component size");

        // Replacing an existing component must not allocate a new dense row.
        if let Ok(existing) = self.dense_index(entity_key) {
            let existing_pos = existing.get() as usize;
            if let NeedsDrop::NonTrivial { drop_fn } = self.meta.needs_drop() {
                // SAFETY: `existing < len` (validated by `dense_index`) keeps the
                // pointer within the initialized prefix of `buf_ptr`.
                unsafe { drop_fn(self.buf_ptr.as_ptr().add(existing_pos * size)) };
            }
            // SAFETY: `existing < len` (validated by `dense_index`) keeps the write
            // within the initialized prefix of `buf_ptr`.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    comp_data.as_ptr(),
                    self.buf_ptr.as_ptr().add(existing_pos * size),
                    size,
                );
            }
            return;
        }

        if self.len() == self.capacity {
            self.reserve(1);
        }
        let dense_index = DenseIndex::from_usize(self.len())
            .expect("SparseSet cannot index more than u32::MAX - 1 dense entries");

        // SAFETY: `dense_index < capacity` keeps the write within the allocation, and
        // `comp_data` length was just checked to equal `size`.
        unsafe {
            std::ptr::copy_nonoverlapping(
                comp_data.as_ptr(),
                self.buf_ptr.as_ptr().add(dense_index.get() as usize * size),
                size,
            );
        }

        self.entity_keys.push(entity_key);
        let entity_pos = entity_key.index.get() as usize;
        if self.entity_to_dense.len() <= entity_pos {
            self.entity_to_dense.resize(entity_pos + 1, None);
        }
        self.entity_to_dense[entity_pos] = Some(dense_index);
    }

    /// Resolves `entity_key` to its dense row in this set.
    pub fn dense_index(&self, entity_key: EntityKey) -> Result<DenseIndex, SparseSetError> {
        let entity_pos = entity_key.index.get() as usize;
        let dense = match self.entity_to_dense.get(entity_pos) {
            None => {
                return Err(SparseSetError::IndexOutOfBounds {
                    index: entity_key.index.get(),
                    bounds: self.entity_to_dense.len(),
                });
            }
            Some(Some(dense)) => *dense,
            Some(None) => {
                return Err(SparseSetError::ComponentNotLive { entity_key });
            }
        };

        let stored = self.entity_keys[dense.get() as usize];
        if stored.generation != entity_key.generation {
            return Err(SparseSetError::GenerationMismatch {
                generation: entity_key.generation,
                expected: stored.generation,
            });
        }
        Ok(dense)
    }

    /// Removes the component owned by `entity_key`.
    ///
    /// This performs a dense `swap_remove`: if the removed row is not the last dense
    /// row, the last row is moved into the removed slot and the owner's dense mapping
    /// is updated accordingly.
    pub fn remove(&mut self, entity_key: EntityKey) -> Result<(), SparseSetError> {
        let remove_dense = self.dense_index(entity_key)?;
        let remove_dense_pos = remove_dense.get() as usize;
        let last_dense_pos =
            self.len().checked_sub(1).expect("cannot remove from an empty sparse set");

        let size = self.meta.layout().size();
        if let NeedsDrop::NonTrivial { drop_fn } = self.meta.needs_drop() {
            // SAFETY: `remove_dense_pos < len` and `drop_fn` is valid for this component type.
            unsafe { drop_fn(self.buf_ptr.as_ptr().add(remove_dense_pos * size)) };
        }

        if remove_dense_pos != last_dense_pos {
            // SAFETY: both source and destination are valid dense rows and do not overlap.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buf_ptr.as_ptr().add(last_dense_pos * size),
                    self.buf_ptr.as_ptr().add(remove_dense_pos * size),
                    size,
                );
            }

            // Keep the entity-to-dense view aligned with the dense swap.
            let moved_entity = self.entity_keys[last_dense_pos];
            self.entity_to_dense[moved_entity.index.get() as usize] = Some(remove_dense);
        }

        let removed_entity = self.entity_keys.swap_remove(remove_dense_pos);
        self.entity_to_dense[removed_entity.index.get() as usize] = None;
        Ok(())
    }

    /// Returns the component bytes owned by `entity_key`.
    pub fn get_component(&self, entity_key: EntityKey) -> Result<&[u8], SparseSetError> {
        let dense = self.dense_index(entity_key)?;
        let size = self.meta.layout().size();
        let start = dense.get() as usize * size;
        // SAFETY: `dense < len` (validated by `dense_index`) keeps the `size`-byte
        // slice within the `len * size`-byte initialized prefix of `buf_ptr`.
        Ok(unsafe { std::slice::from_raw_parts(self.buf_ptr.as_ptr().add(start), size) })
    }

    /// Returns the component bytes owned by `entity_key` as a mutable slice.
    pub fn get_component_mut(
        &mut self,
        entity_key: EntityKey,
    ) -> Result<&mut [u8], SparseSetError> {
        let dense = self.dense_index(entity_key)?;
        let size = self.meta.layout().size();
        let start = dense.get() as usize * size;
        // SAFETY: `dense < len` (validated by `dense_index`) keeps the `size`-byte
        // slice within the `len * size`-byte initialized prefix of `buf_ptr`, and the
        // mutable borrow is exclusive.
        Ok(unsafe { std::slice::from_raw_parts_mut(self.buf_ptr.as_ptr().add(start), size) })
    }

    /// Returns the bytes of the dense component array covering the valid entities: the
    /// first `len` elements.
    pub fn get_components(&self) -> &[u8] {
        let size = self.meta.layout().size();
        // SAFETY: `buf_ptr` is a `capacity`-element allocation of `size`-byte
        // elements; `len <= capacity` keeps the `len * size`-byte slice within it.
        unsafe { std::slice::from_raw_parts(self.buf_ptr.as_ptr(), self.len() * size) }
    }

    /// Returns the bytes of the dense component array covering the valid entities as a
    /// mutable slice: the first `len` elements.
    pub fn get_components_mut(&mut self) -> &mut [u8] {
        let len = self.len() * self.meta.layout().size();
        // SAFETY: `buf_ptr` is a `capacity`-element allocation of `size`-byte
        // elements; `len <= capacity` keeps the `len * size`-byte slice within it.
        unsafe { std::slice::from_raw_parts_mut(self.buf_ptr.as_ptr(), len) }
    }

    /// Returns an iterator over the dense array's valid entities, one component-sized
    /// byte slice per entity.
    pub fn get_component_chunks(&self) -> ChunksExact<'_, u8> {
        self.get_components().chunks_exact(self.meta.layout().size())
    }

    /// Returns a mutable iterator over the dense array's valid entities, one
    /// component-sized byte slice per entity.
    pub fn get_component_chunks_mut(&mut self) -> ChunksExactMut<'_, u8> {
        let size = self.meta.layout().size();
        self.get_components_mut().chunks_exact_mut(size)
    }

    /// Returns a non-null pointer to the start of the dense component array, for placing
    /// new component values.
    ///
    /// # Safety
    ///
    /// The caller must ensure no aliasing references exist and must not read or overwrite
    /// slots beyond `self.len()` by assignment: they are uninitialized. Convert the pointer
    /// with [`NonNull::as_ptr`] for raw pointer operations (write or copy).
    pub unsafe fn get_component_mut_ptr(&mut self) -> NonNull<u8> {
        self.buf_ptr
    }
}

impl Drop for SparseSet {
    fn drop(&mut self) {
        let layout = self.meta.layout();
        let size = layout.size();
        let align = layout.align();
        if let NeedsDrop::NonTrivial { drop_fn } = self.meta.needs_drop() {
            for i in 0..self.len() {
                // SAFETY: drop_fn only consumes valid component slots.
                unsafe {
                    drop_fn(self.buf_ptr.as_ptr().add(i * size));
                }
            }
        }

        if self.capacity == 0 || size == 0 {
            return;
        }
        let layout = Layout::from_size_align(size * self.capacity, align)
            .expect("sparse set buffer layout must be valid");
        unsafe {
            dealloc(self.buf_ptr.as_ptr(), layout);
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SparseSetError {
    #[error("Index {index} is out of bounds for sparse set with {bounds} entries")]
    IndexOutOfBounds { index: u32, bounds: usize },

    #[error("Entity {entity_key:?} does not refer to a live entry")]
    ComponentNotLive { entity_key: EntityKey },

    #[error("Generation {generation:?} does not match the expected {expected:?}")]
    GenerationMismatch {
        generation: EntityGeneration,
        expected: EntityGeneration,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityGeneration, EntityIndex};
    use starforge_reflect::prelude::TypeId;

    fn entity(id: u32) -> EntityKey {
        EntityKey {
            index: EntityIndex::new(id).unwrap(),
            generation: EntityGeneration::new(0).unwrap(),
        }
    }

    fn sparse_set_u32(capacity: usize) -> SparseSet {
        SparseSet::with_capacity(TypeMeta::new::<u32>(), capacity)
    }

    fn entity_stale(id: u32) -> EntityKey {
        EntityKey {
            index: EntityIndex::new(id).unwrap(),
            generation: EntityGeneration::new(1).unwrap(),
        }
    }

    /// A non-trivial component whose drop increments `Tracker`.
    struct Tracked {
        tracker: std::sync::Arc<Tracker>,
    }

    #[derive(Default)]
    struct Tracker {
        drops: std::sync::atomic::AtomicUsize,
    }

    impl Tracker {
        fn new() -> std::sync::Arc<Self> {
            std::sync::Arc::default()
        }

        fn drop_count(&self) -> usize {
            self.drops.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.tracker.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl Tracked {
        /// Raw byte representation of a freshly allocated value, without running
        /// the temporary's drop glue (the sparse set owns the buffer copy).
        fn new_bytes(tracker: &std::sync::Arc<Tracker>) -> [u8; std::mem::size_of::<Tracked>()] {
            let mut uninit = std::mem::MaybeUninit::<Tracked>::uninit();
            // SAFETY: `uninit` is uninitialized and points to enough space for `Tracked`.
            unsafe { std::ptr::write(uninit.as_mut_ptr(), Tracked { tracker: tracker.clone() }) };
            // SAFETY: `Tracked` is a single `Arc`; the interpreter and the sparse
            // set agree on the same layout via `TypeMeta::new::<Tracked>()`.
            unsafe {
                std::slice::from_raw_parts(
                    uninit.as_ptr().cast::<u8>(),
                    std::mem::size_of::<Tracked>(),
                )
                .try_into()
                .unwrap()
            }
        }
    }

    #[test]
    fn with_capacity_pre_allocates_at_least_the_requested_capacity() {
        let set = sparse_set_u32(8);

        assert_eq!(set.len(), 0);
        assert!(set.capacity >= 8);
    }

    #[test]
    fn meta_reports_the_stored_component_type() {
        let set = sparse_set_u32(0);

        assert_eq!(set.meta().id(), TypeId::of::<u32>());
        assert_eq!(set.meta().layout().size(), std::mem::size_of::<u32>());
    }

    #[test]
    fn reserve_grows_capacity_to_fit_additional_elements() {
        let mut set = sparse_set_u32(0);

        set.reserve(3);

        assert!(set.capacity >= 3);
    }

    #[test]
    fn reserve_is_a_no_op_when_capacity_already_suffices() {
        let mut set = sparse_set_u32(8);
        let capacity_before = set.capacity;

        set.reserve(2);

        assert_eq!(set.capacity, capacity_before);
    }

    #[test]
    fn insert_grows_the_buffer_automatically_when_full() {
        let mut set = sparse_set_u32(0);

        for i in 0..5 {
            set.insert(entity(i), &(i * 10).to_ne_bytes());
        }

        assert_eq!(set.len(), 5);
        assert!(set.capacity >= 5);
    }

    #[test]
    fn get_component_returns_bytes_for_all_live_entries() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());
        set.insert(entity(1), &22u32.to_ne_bytes());

        let values: Vec<u32> = set
            .get_components()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_ne_bytes(*c))
            .collect();

        assert_eq!(values, vec![11, 22]);
    }

    #[test]
    fn get_component_mut_allows_in_place_updates() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());

        set.get_components_mut()[0..4].copy_from_slice(&99u32.to_ne_bytes());

        let value = u32::from_ne_bytes(set.get_components()[0..4].try_into().unwrap());
        assert_eq!(value, 99);
    }

    #[test]
    fn get_component_chunks_yields_one_chunk_per_entity() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());
        set.insert(entity(1), &22u32.to_ne_bytes());

        let values: Vec<u32> = set
            .get_component_chunks()
            .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
            .collect();

        assert_eq!(values, vec![11, 22]);
    }

    #[test]
    fn get_component_chunks_mut_allows_in_place_updates() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());
        set.insert(entity(1), &22u32.to_ne_bytes());

        for chunk in set.get_component_chunks_mut() {
            let value = u32::from_ne_bytes(chunk.try_into().unwrap());
            chunk.copy_from_slice(&(value + 1).to_ne_bytes());
        }

        let values: Vec<u32> = set
            .get_component_chunks()
            .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(values, vec![12, 23]);
    }

    #[test]
    fn get_component_mut_ptr_points_at_the_dense_buffer_start() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());

        // SAFETY: writing a single in-bounds `u32` slot that is already initialized.
        unsafe {
            set.get_component_mut_ptr().as_ptr().cast::<u32>().write(42);
        }

        let value = u32::from_ne_bytes(set.get_components()[0..4].try_into().unwrap());
        assert_eq!(value, 42);
    }

    #[test]
    fn dense_index_distinguishes_index_bounds_not_live_and_stale_generation() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(2), &11u32.to_ne_bytes());

        // Index 0 is within `entity_to_dense` length but owns no component.
        assert_eq!(
            set.dense_index(entity(0)),
            Err(SparseSetError::ComponentNotLive { entity_key: entity(0) })
        );
        // Index 4 was never mapped: the sparse array stops at the highest mapped index.
        assert_eq!(
            set.dense_index(entity(4)),
            Err(SparseSetError::IndexOutOfBounds { index: 4, bounds: 3 })
        );
        // Index 2 belongs to generation 0, so generation 1 must be rejected.
        assert_eq!(
            set.dense_index(entity_stale(2)),
            Err(SparseSetError::GenerationMismatch {
                generation: EntityGeneration::new(1).unwrap(),
                expected: EntityGeneration::new(0).unwrap(),
            })
        );
        // The live key resolves.
        assert_eq!(set.dense_index(entity(2)), Ok(DenseIndex::new(0).unwrap()));
    }

    #[test]
    fn insert_replaces_existing_key_in_place_without_growing_dense_array() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());
        set.insert(entity(1), &22u32.to_ne_bytes());

        set.insert(entity(0), &99u32.to_ne_bytes());

        // Replacement reuses the existing dense row: length and mapping stay put.
        assert_eq!(set.len(), 2);
        assert_eq!(set.dense_index(entity(0)), Ok(DenseIndex::new(0).unwrap()));
        assert_eq!(set.dense_index(entity(1)), Ok(DenseIndex::new(1).unwrap()));
        let values: Vec<u32> = set
            .get_components()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_ne_bytes(*c))
            .collect();
        assert_eq!(values, vec![99, 22]);
    }

    #[test]
    fn remove_swaps_last_row_and_updates_entity_to_dense_mapping() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());
        set.insert(entity(1), &22u32.to_ne_bytes());
        set.insert(entity(2), &33u32.to_ne_bytes());

        // Removing the first row moves entity 2 (the last row) into slot 0.
        set.remove(entity(0)).unwrap();

        assert_eq!(set.len(), 2);
        // The moved entity keeps resolving, via its updated mapping.
        assert_eq!(set.entity_keys(), &[entity(2), entity(1)]);
        assert_eq!(set.dense_index(entity(2)), Ok(DenseIndex::new(0).unwrap()));
        assert_eq!(set.dense_index(entity(1)), Ok(DenseIndex::new(1).unwrap()));
        // The removed entity no longer owns a component.
        assert_eq!(
            set.dense_index(entity(0)),
            Err(SparseSetError::ComponentNotLive { entity_key: entity(0) })
        );

        let values: Vec<u32> = set
            .get_components()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_ne_bytes(*c))
            .collect();
        assert_eq!(values, vec![33, 22]);
    }

    #[test]
    fn remove_rejects_missing_and_stale_entities() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(2), &11u32.to_ne_bytes());

        assert_eq!(
            set.remove(entity(0)),
            Err(SparseSetError::ComponentNotLive { entity_key: entity(0) })
        );
        assert_eq!(
            set.remove(entity_stale(2)),
            Err(SparseSetError::GenerationMismatch {
                generation: EntityGeneration::new(1).unwrap(),
                expected: EntityGeneration::new(0).unwrap(),
            })
        );
        // Removal is idempotent for a key that never resolved.
        assert_eq!(
            set.remove(entity(9)),
            Err(SparseSetError::IndexOutOfBounds { index: 9, bounds: 3 })
        );
    }

    #[test]
    fn get_component_reads_and_writes_a_single_entity() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());
        set.insert(entity(1), &22u32.to_ne_bytes());

        assert_eq!(
            u32::from_ne_bytes(set.get_component(entity(1)).unwrap().try_into().unwrap()),
            22
        );

        set.get_component_mut(entity(0)).unwrap().copy_from_slice(&77u32.to_ne_bytes());
        let values: Vec<u32> = set
            .get_components()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_ne_bytes(*c))
            .collect();
        assert_eq!(values, vec![77, 22]);
    }

    #[test]
    fn replacing_a_non_trivial_component_drops_the_old_value() {
        let tracker = Tracker::new();
        let mut set = SparseSet::with_capacity(TypeMeta::new::<Tracked>(), 4);
        set.insert(entity(0), &Tracked::new_bytes(&tracker));
        assert_eq!(tracker.drop_count(), 0);

        // The stored value is dropped in place when replaced by a new value.
        set.insert(entity(0), &Tracked::new_bytes(&tracker));

        assert_eq!(set.len(), 1);
        assert_eq!(tracker.drop_count(), 1);

        // Dropping the sparse set drops the live value.
        drop(set);
        assert_eq!(tracker.drop_count(), 2);
    }

    #[test]
    fn removing_a_non_trivial_component_drops_the_value() {
        let tracker = Tracker::new();
        let mut set = SparseSet::with_capacity(TypeMeta::new::<Tracked>(), 4);
        set.insert(entity(0), &Tracked::new_bytes(&tracker));
        set.insert(entity(1), &Tracked::new_bytes(&tracker));

        set.remove(entity(0)).unwrap();
        drop(set);

        assert_eq!(tracker.drop_count(), 2);
    }
}
