mod header;
mod registry;

use std::{
    alloc::{Layout, dealloc},
    ptr::NonNull,
    slice::{ChunksExact, ChunksExactMut},
};

use crate::prelude::*;

pub use header::{Error, SparseSetHeader};

/// A sparse set storing one component type's dense data alongside a sparse-to-dense
/// index, keyed by entity.
pub struct SparseSet {
    /// Metadata describing the stored component type, frozen at construction time.
    pub header: SparseSetHeader,
    sparse_to_dense: Vec<usize>,
    dense_to_sparse: Vec<usize>,
    retired_sparse: Vec<usize>,
    entity_keys: Vec<EntityKey>,
    buf_ptr: NonNull<u8>,
    /// Number of elements `buf_ptr` was allocated to hold.
    capacity: usize,
}

impl SparseSet {
    /// Returns the number of entities currently stored in the dense array.
    pub fn len(&self) -> usize {
        self.entity_keys.len()
    }

    /// Builds an empty `SparseSet` for `header`'s component type, without allocating a
    /// dense buffer.
    pub fn new(header: SparseSetHeader) -> Self {
        Self {
            header,
            sparse_to_dense: Vec::new(),
            dense_to_sparse: Vec::new(),
            retired_sparse: Vec::new(),
            entity_keys: Vec::new(),
            buf_ptr: NonNull::dangling(),
            capacity: 0,
        }
    }

    /// Builds a `SparseSet` for `header`'s component type with a dense buffer
    /// preallocated to hold `capacity` elements, sized and aligned from the header.
    pub fn with_capacity(header: SparseSetHeader, capacity: usize) -> Self {
        let buf_ptr = if capacity == 0 || header.stride == 0 {
            NonNull::<u8>::dangling()
        } else {
            let layout = Layout::from_size_align(header.stride * capacity, header.align)
                .expect("sparse set buffer layout must be valid");
            NonNull::new(unsafe { std::alloc::alloc(layout) })
                .unwrap_or_else(|| std::alloc::handle_alloc_error(layout))
        };

        Self {
            header,
            sparse_to_dense: Vec::with_capacity(capacity),
            dense_to_sparse: Vec::with_capacity(capacity),
            retired_sparse: Vec::new(),
            entity_keys: Vec::with_capacity(capacity),
            buf_ptr,
            capacity,
        }
    }

    /// Inserts each entity/component pair from `entity_keys` and `components` (parallel,
    /// same length and order) into the dense array. Reuses a retired sparse slot per
    /// insertion when available, otherwise allocates a new one.
    ///
    /// Returns the sparse index assigned to each inserted entity, in the same order as
    /// `entity_keys`.
    ///
    /// # Panics
    ///
    /// Panics if `entity_keys` and `components` differ in length, if a component slice's
    /// length does not match the header's stride, or if inserting would exceed the dense
    /// buffer's capacity.
    pub fn insert(
        &mut self,
        entity_keys: &[EntityKey],
        components: ChunksExactMut<'_, u8>,
    ) -> Vec<usize> {
        assert_eq!(
            components.len(),
            entity_keys.len(),
            "entity_keys and components must have the same length"
        );

        let mut sparse_indices = Vec::with_capacity(entity_keys.len());
        for (&entity_key, component) in entity_keys.iter().zip(components) {
            assert_eq!(
                component.len(),
                self.header.stride,
                "component size must match the header's stride"
            );
            let dense_index = self.len();
            assert!(dense_index < self.capacity, "sparse set buffer is at capacity");

            // SAFETY: `dense_index < capacity` keeps the write within the allocation, and
            // `component`'s length was just checked to equal `stride`.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    component.as_ptr(),
                    self.buf_ptr.as_ptr().add(dense_index * self.header.stride),
                    self.header.stride,
                );
            }

            self.entity_keys.push(entity_key);

            let sparse_index = if let Some(sparse_index) = self.retired_sparse.pop() {
                self.sparse_to_dense[sparse_index] = dense_index;
                sparse_index
            } else {
                let sparse_index = self.sparse_to_dense.len();
                self.sparse_to_dense.push(dense_index);
                sparse_index
            };

            self.dense_to_sparse.push(sparse_index);
            sparse_indices.push(sparse_index);
        }

        sparse_indices
    }

    /// Returns the bytes of the dense component array covering the valid entities: the
    /// first `len` elements.
    pub fn get_component(&self) -> &[u8] {
        // SAFETY: `buf_ptr` is a `capacity`-element allocation of `stride`-byte
        // elements; `len <= capacity` keeps the `len * stride`-byte slice within it.
        unsafe {
            std::slice::from_raw_parts(self.buf_ptr.as_ptr(), self.len() * self.header.stride)
        }
    }

    /// Returns the bytes of the dense component array covering the valid entities as a
    /// mutable slice: the first `len` elements.
    pub fn get_component_mut(&mut self) -> &mut [u8] {
        let len = self.len() * self.header.stride;
        // SAFETY: `buf_ptr` is a `capacity`-element allocation of `stride`-byte
        // elements; `len <= capacity` keeps the `len * stride`-byte slice within it.
        unsafe { std::slice::from_raw_parts_mut(self.buf_ptr.as_ptr(), len) }
    }

    /// Returns an iterator over the dense array's valid entities, one component-sized
    /// byte slice per entity.
    pub fn get_component_chunks(&self) -> ChunksExact<'_, u8> {
        self.get_component().chunks_exact(self.header.stride)
    }

    /// Returns a mutable iterator over the dense array's valid entities, one
    /// component-sized byte slice per entity.
    pub fn get_component_chunks_mut(&mut self) -> ChunksExactMut<'_, u8> {
        let stride = self.header.stride;
        self.get_component_mut().chunks_exact_mut(stride)
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
        if let ComponentKind::NonTrivial { drop_fn } = self.header.comp_kind {
            for i in 0..self.len() {
                unsafe {
                    drop_fn(self.buf_ptr.as_ptr().add(i * self.header.stride));
                }
            }
        }

        if self.capacity == 0 || self.header.stride == 0 {
            return;
        }
        let layout = Layout::from_size_align(self.header.stride * self.capacity, self.header.align)
            .expect("sparse set buffer layout must be valid");
        unsafe {
            dealloc(self.buf_ptr.as_ptr(), layout);
        }
    }
}
