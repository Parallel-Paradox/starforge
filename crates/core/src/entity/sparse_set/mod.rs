mod registry;

use std::{
    alloc::{Layout, dealloc},
    ptr::NonNull,
    slice::{ChunksExact, ChunksExactMut},
};

use crate::prelude::EntityKey;
use starforge_macro::Deref;
use starforge_reflect::{basic::meta::NeedsDrop, prelude::TypeMeta};

pub use registry::{
    Error as RegistryError, SparseSetGeneration, SparseSetIndex, SparseSetKey, SparseSetRegistry,
};

use nonmax::NonMaxU32;

/// A sparse set storing one component type's dense data alongside a sparse-to-dense
/// index, keyed by entity.
pub struct SparseSet {
    meta: TypeMeta,
    sparse_to_dense: Vec<DenseIndex>,
    dense_to_sparse: Vec<SparseIndex>,
    retired_sparse: Vec<SparseIndex>,
    entity_keys: Vec<EntityKey>,
    buf_ptr: NonNull<u8>,
    /// Number of elements `buf_ptr` was allocated to hold.
    capacity: usize,
}

/// Non-`u32::MAX` slot index into a [`SparseSet`]'s sparse array, keyed by entity.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct SparseIndex(NonMaxU32);

/// Non-`u32::MAX` slot index into a [`SparseSet`]'s dense array.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct DenseIndex(NonMaxU32);

impl SparseIndex {
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

    /// Builds an empty `SparseSet` for `meta`'s component type, without allocating a
    /// dense buffer.
    pub fn new(meta: TypeMeta) -> Self {
        Self {
            meta,
            sparse_to_dense: Vec::new(),
            dense_to_sparse: Vec::new(),
            retired_sparse: Vec::new(),
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
        self.sparse_to_dense.reserve(additional);
        self.dense_to_sparse.reserve(additional);
        self.entity_keys.reserve(additional);
    }

    /// Inserts an entity/component pair into the dense array. Reuses a retired sparse
    /// slot when available, otherwise allocates a new one.
    ///
    /// Grows the dense buffer (doubling capacity) if it is full.
    ///
    /// Returns the sparse index assigned to the inserted entity.
    ///
    /// # Panics
    ///
    /// Panics if `comp_data` length does not match the meta's component size.
    pub fn insert(&mut self, entity_key: EntityKey, comp_data: &[u8]) -> SparseIndex {
        let size = self.meta.layout().size();
        assert_eq!(comp_data.len(), size, "component data length must match the component size");
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

        let sparse_index = if let Some(sparse_index) = self.retired_sparse.pop() {
            self.sparse_to_dense[sparse_index.get() as usize] = dense_index;
            sparse_index
        } else {
            let sparse_index = SparseIndex::from_usize(self.sparse_to_dense.len())
                .expect("SparseSet cannot index more than u32::MAX - 1 sparse entries");
            self.sparse_to_dense.push(dense_index);
            sparse_index
        };

        self.dense_to_sparse.push(sparse_index);
        sparse_index
    }

    /// Removes the entity/component pair addressed by `sparse_index`.
    ///
    /// This performs a dense `swap_remove`: if the removed row is not the last dense
    /// row, the last row is moved into the removed slot and all sparse/dense mappings
    /// are updated accordingly. The freed sparse slot is retired and may be reused by
    /// subsequent insertions.
    ///
    /// # Panics
    ///
    /// Panics if `sparse_index` is out of bounds or does not refer to a live entry.
    pub fn remove(&mut self, sparse_index: SparseIndex) {
        let sparse_pos = sparse_index.get() as usize;
        let remove_dense = self.sparse_to_dense[sparse_pos];
        let remove_dense_pos = remove_dense.get() as usize;
        let last_dense_pos =
            self.len().checked_sub(1).expect("cannot remove from an empty sparse set");

        assert_eq!(
            self.dense_to_sparse[remove_dense_pos], sparse_index,
            "sparse index does not refer to a live entry"
        );

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

            let moved_sparse = self.dense_to_sparse[last_dense_pos];
            self.sparse_to_dense[moved_sparse.get() as usize] = remove_dense;
        }

        self.entity_keys.swap_remove(remove_dense_pos);
        self.dense_to_sparse.swap_remove(remove_dense_pos);
        self.retired_sparse.push(sparse_index);
    }

    /// Returns the bytes of the dense component array covering the valid entities: the
    /// first `len` elements.
    pub fn get_component(&self) -> &[u8] {
        let size = self.meta.layout().size();
        // SAFETY: `buf_ptr` is a `capacity`-element allocation of `size`-byte
        // elements; `len <= capacity` keeps the `len * size`-byte slice within it.
        unsafe { std::slice::from_raw_parts(self.buf_ptr.as_ptr(), self.len() * size) }
    }

    /// Returns the bytes of the dense component array covering the valid entities as a
    /// mutable slice: the first `len` elements.
    pub fn get_component_mut(&mut self) -> &mut [u8] {
        let len = self.len() * self.meta.layout().size();
        // SAFETY: `buf_ptr` is a `capacity`-element allocation of `size`-byte
        // elements; `len <= capacity` keeps the `len * size`-byte slice within it.
        unsafe { std::slice::from_raw_parts_mut(self.buf_ptr.as_ptr(), len) }
    }

    /// Returns an iterator over the dense array's valid entities, one component-sized
    /// byte slice per entity.
    pub fn get_component_chunks(&self) -> ChunksExact<'_, u8> {
        self.get_component().chunks_exact(self.meta.layout().size())
    }

    /// Returns a mutable iterator over the dense array's valid entities, one
    /// component-sized byte slice per entity.
    pub fn get_component_chunks_mut(&mut self) -> ChunksExactMut<'_, u8> {
        let size = self.meta.layout().size();
        self.get_component_mut().chunks_exact_mut(size)
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

    #[test]
    fn remove_swaps_last_dense_row_and_updates_mappings() {
        let mut set = sparse_set_u32(4);
        let s0 = set.insert(entity(0), &11u32.to_ne_bytes());
        let s1 = set.insert(entity(1), &22u32.to_ne_bytes());

        set.remove(s0);

        assert_eq!(set.len(), 1);
        assert_eq!(set.entity_keys[0], entity(1));
        assert_eq!(set.dense_to_sparse[0], s1);
        assert_eq!(set.sparse_to_dense[s1.get() as usize], DenseIndex::new(0).unwrap());
        assert_eq!(set.retired_sparse, vec![s0]);
    }

    #[test]
    fn insert_reuses_retired_sparse_slot_after_remove() {
        let mut set = sparse_set_u32(4);
        let s0 = set.insert(entity(0), &11u32.to_ne_bytes());
        let _s1 = set.insert(entity(1), &22u32.to_ne_bytes());

        set.remove(s0);
        let reused = set.insert(entity(2), &33u32.to_ne_bytes());

        assert_eq!(reused, s0);
    }

    #[test]
    fn new_starts_empty_without_allocating() {
        let set = sparse_set_u32(0);

        assert_eq!(set.len(), 0);
        assert_eq!(set.capacity, 0);
        assert!(set.entity_keys().is_empty());
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
            .get_component()
            .chunks_exact(4)
            .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
            .collect();

        assert_eq!(values, vec![11, 22]);
    }

    #[test]
    fn get_component_mut_allows_in_place_updates() {
        let mut set = sparse_set_u32(4);
        set.insert(entity(0), &11u32.to_ne_bytes());

        set.get_component_mut()[0..4].copy_from_slice(&99u32.to_ne_bytes());

        let value = u32::from_ne_bytes(set.get_component()[0..4].try_into().unwrap());
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

        let value = u32::from_ne_bytes(set.get_component()[0..4].try_into().unwrap());
        assert_eq!(value, 42);
    }
}
