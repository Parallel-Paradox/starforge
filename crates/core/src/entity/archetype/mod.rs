mod chunk;
mod layout;
mod meta;
mod registry;

use std::sync::Arc;
use thiserror::Error;

pub use chunk::ArchetypeChunk;
pub use layout::{ArchetypeChunkLayout, Error as LayoutError};
pub use meta::{ArchetypeMeta, ColumnEntry};
pub use registry::{
    ArchetypeGeneration, ArchetypeIndex, ArchetypeKey, ArchetypeRegistry, ArchetypeSignature,
    Error as RegistryError,
};

use crate::entity::EntityKey;

/// A family of chunks sharing one [`ArchetypeMeta`] and one component layout, storing
/// entities of the same component set in contiguous buffers.
///
/// Entities are spread across [`ArchetypeChunk`]s; capacity grows on demand by doubling
/// the first chunk or appending new chunks, see [`Archetype::reserve`].
pub struct Archetype {
    /// The frozen metadata describing the component columns shared by every chunk.
    pub meta: Arc<ArchetypeMeta>,
    chunks: Vec<ArchetypeChunk>,
    entity_keys: Vec<EntityKey>,
}

impl Archetype {
    /// Number of live entities across all chunks.
    pub fn len(&self) -> usize {
        self.entity_keys.len()
    }

    /// Returns `true` if the archetype holds no live entities.
    pub fn is_empty(&self) -> bool {
        self.entity_keys.is_empty()
    }

    /// Returns all live entity keys in row order.
    pub fn get_entity_keys(&self) -> &[EntityKey] {
        &self.entity_keys
    }

    /// Returns the bytes of one component at `column` for `row`.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of bounds of live entities, or if `column`
    /// is out of bounds of the archetype columns.
    pub fn get_column_row(&self, column: usize, row: usize) -> &[u8] {
        let (chunk_index, row_in_chunk) = self.locate_row(row);
        let chunk = &self.chunks[chunk_index];
        let stride = chunk.meta.column_size(column);
        let start = row_in_chunk * stride;
        let end = start + stride;
        &chunk.get_column(column)[start..end]
    }

    /// Returns mutable bytes of one component at `column` for `row`.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of bounds of live entities, or if `column`
    /// is out of bounds of the archetype columns.
    pub fn get_column_row_mut(&mut self, column: usize, row: usize) -> &mut [u8] {
        let (chunk_index, row_in_chunk) = self.locate_row(row);
        let chunk = &mut self.chunks[chunk_index];
        let stride = chunk.meta.column_size(column);
        let start = row_in_chunk * stride;
        let end = start + stride;
        &mut chunk.get_column_mut(column)[start..end]
    }

    /// Maps a global row index to `(chunk_index, row_in_chunk)` using each chunk's
    /// live length.
    fn locate_row(&self, row: usize) -> (usize, usize) {
        assert!(row < self.len(), "row out of bounds");

        let mut base = 0;
        for (chunk_index, chunk) in self.chunks.iter().enumerate() {
            let next = base + chunk.len();
            if row < next {
                return (chunk_index, row - base);
            }
            base = next;
        }

        unreachable!("row is within total length but no chunk contains it");
    }

    /// Creates an empty archetype backed by a single chunk whose layout holds exactly
    /// one entity; the chunk grows on demand via [`Archetype::reserve`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::ArchetypeTooLarge`] if a single entity's bytes (all columns)
    /// exceed [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    pub fn new(meta: Arc<ArchetypeMeta>) -> Result<Self, Error> {
        let layout = ArchetypeChunkLayout::with_capacity(&meta, 1).map_err(|e| match e {
            LayoutError::BufferTooLarge { buffer_size, max } => {
                Error::ArchetypeTooLarge { per_entity_size: buffer_size, max }
            }
        })?;
        let layout = Arc::new(layout);
        let chunk = ArchetypeChunk::new(meta.clone(), layout);
        Ok(Self { meta, chunks: vec![chunk], entity_keys: Vec::new() })
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.entity_keys.clear();
    }

    /// Reserves capacity for at least `additional` more entities, so subsequent insertions
    /// do not reallocate.
    ///
    /// While only the first chunk exists, its capacity is doubled (capped at the maximum
    /// buffer size) and the live data is relocated into the grown chunk; once the first
    /// chunk can no longer grow, new chunks sharing its layout are appended until the
    /// requested capacity is covered.
    pub fn reserve(&mut self, additional: usize) {
        // Keep entity key storage growth in sync with chunk capacity planning.
        self.entity_keys.reserve(additional);

        // SAFETY: `self.chunks` is guaranteed to be non-empty.
        let last = self.chunks.len() - 1;

        // Enough space in the last chunk, no need to allocate a new one.
        if self.chunks[last].len() + additional <= self.chunks[last].layout.capacity() {
            return;
        }

        // If there is only one chunk, try to double the capacity of this chunk,
        // return until it is large enough.
        // If we exceeds the maximum allowed size when extending, reset layout with
        // maximum buffer size. If even that is not enough, fall through to appending
        // chunks.
        if last == 0 && self.grow_first_chunk(additional) {
            return;
        }

        // If chunks len is greater than 1, just attach some new chunks with the
        // same layout as the last one until there is enough space.
        let layout = self.chunks[last].layout.clone();
        let capacity = layout.capacity();
        let free = capacity - self.chunks[last].len();
        let mut remaining = additional.saturating_sub(free);
        while remaining > 0 {
            self.chunks.push(ArchetypeChunk::new(self.meta.clone(), layout.clone()));
            remaining = remaining.saturating_sub(capacity);
        }
    }

    /// Grows the first chunk to hold at least `len + additional` entities: doubles its
    /// capacity until it fits, capped at the maximum buffer size, then moves the live
    /// data into the new buffer.
    ///
    /// Returns `true` if the grown chunk alone has enough room; `false` if the maximum
    /// buffer size was reached first and the caller must append more chunks.
    fn grow_first_chunk(&mut self, additional: usize) -> bool {
        let required = self.chunks[0].len() + additional;
        let new_layout =
            Self::grow_layout_to_fit(&self.meta, self.chunks[0].layout.capacity(), required);

        let mut new_chunk = ArchetypeChunk::new(self.meta.clone(), new_layout.clone());
        // SAFETY: `new_chunk` shares `meta` with `self.chunks[0]` and its layout can
        // hold `required >= old_len` entities.
        unsafe {
            self.chunks[0].move_data_into(&mut new_chunk);
        }
        self.chunks[0] = new_chunk;

        new_layout.capacity() >= required
    }

    /// Builds a layout holding at least `required` entities by doubling `capacity`,
    /// capped at the maximum buffer size: once doubling would exceed it, the layout is
    /// reset to the largest one that fits.
    fn grow_layout_to_fit(
        meta: &ArchetypeMeta,
        capacity: usize,
        required: usize,
    ) -> Arc<ArchetypeChunkLayout> {
        let mut capacity = capacity.saturating_mul(2);
        loop {
            match ArchetypeChunkLayout::with_capacity(meta, capacity) {
                Ok(layout) if layout.capacity() >= required => return Arc::new(layout),
                Ok(layout) => capacity = layout.capacity().saturating_mul(2),
                // Doubling exceeded the maximum buffer size: reset to the largest
                // layout that fits. `Archetype::new` guarantees a single entity fits,
                // so capacity must be at least 1.
                Err(LayoutError::BufferTooLarge { .. }) => {
                    return Arc::new(
                        ArchetypeChunkLayout::with_buffer_size(
                            meta,
                            ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE,
                        )
                        .unwrap(),
                    );
                }
            }
        }
    }
}

/// Errors returned when building an [`Archetype`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A single entity's bytes exceed the maximum buffer size.
    #[error("Size per entity ({per_entity_size}) exceeds maximum allowed ({max})")]
    ArchetypeTooLarge { per_entity_size: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityGeneration, EntityIndex};
    use crate::macros::Component;
    use crate::prelude::{ComponentKey, ComponentRegistry};
    use starforge_reflect::basic::meta::NeedsDrop;
    use starforge_reflect::prelude::{TypeId, TypeMeta, TypeName};
    use std::alloc::Layout;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test-local drop counters. Each test owns its own `Arc<Tracker>`, so tests never
    /// share mutable state and can run in parallel.
    #[derive(Default)]
    struct Tracker {
        count: AtomicUsize,
        sum: AtomicUsize,
    }

    impl Tracker {
        fn new() -> Arc<Self> {
            Arc::default()
        }
    }

    /// A non-trivial component whose drops are counted on the [`Tracker`] it holds,
    /// to verify relocation does not drop components twice.
    #[derive(Component)]
    struct Tracked {
        value: u64,
        tracker: Arc<Tracker>,
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.tracker.count.fetch_add(1, Ordering::SeqCst);
            self.tracker.sum.fetch_add(self.value as usize, Ordering::SeqCst);
        }
    }

    /// Script column (id, size, align) triples registered by [`TestContext::mock`], in
    /// registration order.
    const COLUMNS: [(TypeId, usize, usize); 2] =
        [(TypeId::of_script(1), 8, 8), (TypeId::of_script(2), 4, 4)];

    /// Owns the registry backing the columns under test, so keys derived from it stay
    /// resolvable for the lifetime of the context.
    struct TestContext {
        comp_reg: ComponentRegistry,
    }

    impl TestContext {
        /// Builds a context holding the [`COLUMNS`] registrations, in registration order.
        pub fn mock() -> Self {
            let mut comp_reg = ComponentRegistry::default();
            for (id, size, align) in COLUMNS {
                comp_reg.register(TypeMeta::new_impl(
                    id,
                    TypeName::of_script("test"),
                    NeedsDrop::Trivial,
                    Layout::from_size_align(size, align).unwrap(),
                ));
            }
            Self { comp_reg }
        }

        /// Builds a context holding a single wide (8000-byte) column, whose maximum
        /// buffer size holds only two entities.
        pub fn mock_wide() -> Self {
            let mut comp_reg = ComponentRegistry::default();
            let id = TypeId::of_script(1);
            comp_reg.register(TypeMeta::new_impl(
                id,
                TypeName::of_script("wide"),
                NeedsDrop::Trivial,
                Layout::from_size_align(8000, 8).unwrap(),
            ));
            Self { comp_reg }
        }

        /// Builds a context holding only the non-trivial `Tracked` component.
        pub fn mock_tracked() -> Self {
            let mut comp_reg = ComponentRegistry::default();
            comp_reg.register(TypeMeta::new::<Tracked>());
            Self { comp_reg }
        }

        pub fn column(&self, id: TypeId) -> ComponentKey {
            *self.comp_reg.id_to_key(&id).unwrap()
        }

        /// Builds an `ArchetypeMeta` over [`COLUMNS`], in registration order.
        pub fn meta(&self) -> ArchetypeMeta {
            self.meta_with(&COLUMNS.map(|(id, _, _)| id))
        }

        /// Builds an `ArchetypeMeta` over the given ids, in the given order.
        pub fn meta_with(&self, ids: &[TypeId]) -> ArchetypeMeta {
            let keys: Vec<ComponentKey> = ids.iter().map(|id| self.column(*id)).collect();
            ArchetypeMeta::new(&keys, &self.comp_reg).unwrap()
        }
    }

    fn entity_key(index: u32) -> EntityKey {
        EntityKey {
            index: EntityIndex::new(index).unwrap(),
            generation: EntityGeneration::new(0).unwrap(),
        }
    }

    /// `reserve` on the first chunk doubles its capacity and relocates the live
    /// column data into the new buffer.
    #[test]
    fn reserve_grows_first_chunk_and_preserves_data() {
        let ctx = TestContext::mock();
        let mut archetype = Archetype::new(Arc::new(ctx.meta())).unwrap();
        assert_eq!(archetype.chunks.len(), 1);
        assert_eq!(archetype.chunks[0].layout.capacity(), 1);

        // The chunked accessor cover exactly the live rows, so publish `set_len(1)`
        // before writing through them. Both columns are trivial, so a byte copy into
        // each component-sized slot is safe.
        unsafe {
            archetype.chunks[0].set_len(1);
        }
        archetype.chunks[0]
            .get_column_chunks_mut(0)
            .next()
            .unwrap()
            .copy_from_slice(&0x1122_3344_5566_7788u64.to_ne_bytes());
        archetype.chunks[0]
            .get_column_chunks_mut(1)
            .next()
            .unwrap()
            .copy_from_slice(&0xDEAD_BEEFu32.to_ne_bytes());

        // 1 + 10 = 11 entities needed: doubling goes 1 -> 2 -> 4 -> 8 -> 16.
        archetype.reserve(10);

        assert_eq!(archetype.chunks.len(), 1);
        assert_eq!(archetype.chunks[0].layout.capacity(), 16);
        assert_eq!(archetype.chunks[0].len(), 1);

        // The relocated column data survived the move.
        assert_eq!(
            u64::from_ne_bytes(
                archetype.chunks[0].get_column_chunks(0).next().unwrap().try_into().unwrap()
            ),
            0x1122_3344_5566_7788
        );
        assert_eq!(
            u32::from_ne_bytes(
                archetype.chunks[0].get_column_chunks(1).next().unwrap().try_into().unwrap()
            ),
            0xDEAD_BEEF
        );
    }

    /// Once the first chunk reaches the maximum buffer size, `reserve` appends new
    /// chunks sharing that layout until there is enough room.
    #[test]
    fn reserve_appends_chunks_after_first_chunk_maxes_out() {
        // A single 8000-byte column: the maximum
        // buffer size holds only two entities.
        let ctx = TestContext::mock_wide();
        let meta = ctx.meta_with(&[TypeId::of_script(1)]);

        let mut archetype = Archetype::new(Arc::new(meta)).unwrap();
        // 100 entities needed: the first chunk grows to max (capacity 2) and then 49
        // more chunks are appended to cover the remaining 98.
        archetype.reserve(100);

        assert_eq!(archetype.chunks.len(), 50);
        for chunk in &archetype.chunks {
            assert_eq!(chunk.layout.capacity(), 2);
        }
        assert!(Arc::ptr_eq(&archetype.chunks[0].layout, &archetype.chunks[49].layout));
    }

    /// Relocating a chunk must not drop the moved non-trivial components: the old
    /// chunk's length is zeroed before it is dropped, so each component is dropped
    /// exactly once when the archetype itself is dropped.
    #[test]
    fn reserve_moves_non_trivial_components_without_double_drop() {
        let tracker = Tracker::new();

        let ctx = TestContext::mock_tracked();
        let meta = ctx.meta_with(&[TypeId::of::<Tracked>()]);

        let mut archetype = Archetype::new(Arc::new(meta)).unwrap();
        // Publish the row first: `get_column_chunks_mut` covers only live rows. The
        // column is non-trivial, so `ptr::write` moves the value into the
        // uninitialized slot without dropping garbage.
        unsafe {
            archetype.chunks[0].set_len(1);
        }
        let slot = archetype.chunks[0].get_column_chunks_mut(0).next().unwrap();
        // SAFETY: the slot is the single live, uninitialized `Tracked` slot.
        unsafe {
            std::ptr::write(
                slot.as_mut_ptr().cast::<Tracked>(),
                Tracked { value: 42, tracker: tracker.clone() },
            );
        }

        archetype.reserve(10);
        assert_eq!(archetype.chunks.len(), 1);
        assert_eq!(archetype.chunks[0].len(), 1);
        // The move itself does not drop anything.
        assert_eq!(tracker.count.load(Ordering::SeqCst), 0);

        drop(archetype);
        // Dropped exactly once, not twice (the old buffer's length was zeroed).
        assert_eq!(tracker.count.load(Ordering::SeqCst), 1);
        assert_eq!(tracker.sum.load(Ordering::SeqCst), 42);
    }

    #[test]
    fn reserve_grows_entity_keys_capacity() {
        let ctx = TestContext::mock();
        let mut archetype = Archetype::new(Arc::new(ctx.meta())).unwrap();
        let before = archetype.entity_keys.capacity();

        archetype.reserve(32);

        assert!(archetype.entity_keys.capacity() >= before.saturating_add(32));
    }

    #[test]
    fn get_column_row_reads_and_writes_in_single_chunk() {
        let ctx = TestContext::mock();
        let mut archetype = Archetype::new(Arc::new(ctx.meta())).unwrap();

        archetype.reserve(3);

        // Keep logical row count in sync with `ArchetypeChunk::len` for this test.
        archetype.entity_keys.resize(3, entity_key(0));
        unsafe {
            archetype.chunks[0].set_len(3);
        }

        // Fill rows in column 0 as u64 values.
        for (i, slot) in archetype.chunks[0].get_column_chunks_mut(0).enumerate() {
            slot.copy_from_slice(&((i as u64 + 1) * 10).to_ne_bytes());
        }

        let row = 1usize;
        assert_eq!(u64::from_ne_bytes(archetype.get_column_row(0, row).try_into().unwrap()), 20);

        archetype.get_column_row_mut(0, row).copy_from_slice(&99u64.to_ne_bytes());
        assert_eq!(u64::from_ne_bytes(archetype.get_column_row(0, row).try_into().unwrap()), 99);
    }

    #[test]
    fn get_column_row_reads_across_chunks() {
        let ctx = TestContext::mock_wide();
        let meta = ctx.meta_with(&[TypeId::of_script(1)]);
        let mut archetype = Archetype::new(Arc::new(meta)).unwrap();

        // First chunk capacity becomes 2 at max buffer size; reserve appends chunks.
        archetype.reserve(3);

        archetype.entity_keys.resize(4, entity_key(0));
        unsafe {
            archetype.chunks[0].set_len(2);
            archetype.chunks[1].set_len(2);
        }

        // Each row writes a distinct u64 prefix in its 8000-byte slot.
        for i in 0..4 {
            let row = i;
            let slot = archetype.get_column_row_mut(0, row);
            slot[..8].copy_from_slice(&((i as u64 + 1) * 111).to_ne_bytes());
        }

        for i in 0..4 {
            let row = i;
            let slot = archetype.get_column_row(0, row);
            assert_eq!(u64::from_ne_bytes(slot[..8].try_into().unwrap()), (i as u64 + 1) * 111);
        }
    }

    #[test]
    #[should_panic(expected = "row out of bounds")]
    fn get_column_row_panics_when_row_out_of_bounds() {
        let ctx = TestContext::mock();
        let archetype = Archetype::new(Arc::new(ctx.meta())).unwrap();
        let row = 0usize;
        let _ = archetype.get_column_row(0, row);
    }
}
