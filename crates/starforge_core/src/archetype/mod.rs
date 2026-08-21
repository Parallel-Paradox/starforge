mod chunk;
mod layout;
mod meta;
mod registry;

use std::sync::Arc;
use thiserror::Error;

pub use chunk::ArchetypeChunk;
pub use layout::{ArchetypeChunkLayout, Error as LayoutError};
pub use meta::{ArchetypeMeta, ColumnEntry, Error as MetaError};
pub use registry::{ArchetypeKey, ArchetypeRegistry, ArchetypeSignature, Error as RegistryError};

/// A family of chunks sharing one [`ArchetypeMeta`] and one component layout, storing
/// entities of the same component set in contiguous buffers.
///
/// Entities are spread across [`ArchetypeChunk`]s; capacity grows on demand by doubling
/// the first chunk or appending new chunks, see [`Archetype::reserve`].
pub struct Archetype {
    /// The frozen metadata describing the component columns shared by every chunk.
    pub meta: Arc<ArchetypeMeta>,
    chunks: Vec<ArchetypeChunk>,
    len: usize,
}

impl Archetype {
    /// Number of live entities across all chunks.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Creates an empty archetype backed by a single chunk whose layout holds exactly
    /// one entity; the chunk grows on demand via [`Archetype::reserve`].
    ///
    /// # Errors
    ///
    /// Returns [`ArchetypeError::ArchetypeTooLarge`] if a single entity's bytes (all columns
    /// plus the entity key) exceed [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    pub fn new(meta: Arc<ArchetypeMeta>) -> Result<Self, ArchetypeError> {
        let layout = ArchetypeChunkLayout::with_capacity(&meta, 1).map_err(|e| match e {
            LayoutError::BufferTooLarge { buffer_size, max } => {
                ArchetypeError::ArchetypeTooLarge { per_entity_size: buffer_size, max }
            }
        })?;
        let layout = Arc::new(layout);
        let chunk = ArchetypeChunk::new(meta.clone(), layout);
        Ok(Self { meta, chunks: vec![chunk], len: 0 })
    }

    /// Reserves capacity for at least `additional` more entities, so subsequent insertions
    /// do not reallocate.
    ///
    /// While only the first chunk exists, its capacity is doubled (capped at the maximum
    /// buffer size) and the live data is relocated into the grown chunk; once the first
    /// chunk can no longer grow, new chunks sharing its layout are appended until the
    /// requested capacity is covered.
    pub fn reserve(&mut self, additional: usize) {
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
pub enum ArchetypeError {
    /// A single entity's bytes exceed the maximum buffer size.
    #[error("Size per entity ({per_entity_size}) exceeds maximum allowed ({max})")]
    ArchetypeTooLarge { per_entity_size: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::{
        archetype::ColumnEntry,
        component::{ComponentKind, ComponentMeta, ComponentRegistry},
        entity::EntityKey,
        macros::Component,
        types::{TypeId, TypeMeta, TypeRegistry},
    };

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

    /// Script columns registered by [`TestContext::mock`], in registration order.
    const COLUMNS: [TypeMeta; 2] = [
        TypeMeta::new(TypeId::of_script(1), 8, 8, "column_0"),
        TypeMeta::new(TypeId::of_script(2), 4, 4, "column_1"),
    ];

    /// Owns the registries backing the columns under test, so keys derived from them stay
    /// resolvable for the lifetime of the context.
    struct TestContext {
        type_reg: TypeRegistry,
        comp_reg: ComponentRegistry,
    }

    impl TestContext {
        /// Builds a context holding the [`COLUMNS`] registrations, in registration order.
        pub fn mock() -> Self {
            let mut type_reg = TypeRegistry::default();
            let mut comp_reg = ComponentRegistry::default();

            for column in &COLUMNS {
                type_reg.register(column.clone());
                comp_reg.register(ComponentMeta::new(column.id, ComponentKind::Trivial));
            }

            Self { type_reg, comp_reg }
        }

        /// Builds a context holding a single wide (8000-byte) column, whose maximum
        /// buffer size holds only two entities.
        pub fn mock_wide() -> Self {
            let mut type_reg = TypeRegistry::default();
            let mut comp_reg = ComponentRegistry::default();
            let id = TypeId::of_script(1);
            type_reg.register(TypeMeta::new(id, 8000, 8, "wide"));
            comp_reg.register(ComponentMeta::new(id, ComponentKind::Trivial));
            Self { type_reg, comp_reg }
        }

        /// Builds a context holding only the non-trivial `Tracked` component.
        pub fn mock_tracked() -> Self {
            let mut type_reg = TypeRegistry::default();
            let mut comp_reg = ComponentRegistry::default();
            type_reg.register(TypeMeta::of::<Tracked>());
            comp_reg.register(ComponentMeta::of::<Tracked>());
            Self { type_reg, comp_reg }
        }

        pub fn column(&self, id: TypeId) -> ColumnEntry {
            let type_key = *self.type_reg.id_to_key(&id).unwrap();
            let comp_key = *self.comp_reg.id_to_key(&id).unwrap();
            let type_meta = self.type_reg.key_to_meta(&type_key).unwrap();
            let comp_meta = self.comp_reg.key_to_meta(&comp_key).unwrap();
            ColumnEntry::new(type_key, comp_key, type_meta, comp_meta)
        }

        /// Builds an `ArchetypeMeta` over [`COLUMNS`], in registration order.
        pub fn meta(&self) -> ArchetypeMeta {
            ArchetypeMeta::new(
                COLUMNS.iter().map(|c| self.column(c.id)).collect(),
                &self.type_reg,
                &self.comp_reg,
            )
            .unwrap()
        }

        /// Builds an `ArchetypeMeta` over the given ids, in the given order.
        pub fn meta_with(&self, ids: &[TypeId]) -> ArchetypeMeta {
            ArchetypeMeta::new(
                ids.iter().map(|id| self.column(*id)).collect(),
                &self.type_reg,
                &self.comp_reg,
            )
            .unwrap()
        }
    }

    /// `reserve` on the first chunk doubles its capacity and relocates the live data
    /// (entity keys and columns) into the new buffer.
    #[test]
    fn reserve_grows_first_chunk_and_preserves_data() {
        let ctx = TestContext::mock();
        let mut archetype = Archetype::new(Arc::new(ctx.meta())).unwrap();
        assert_eq!(archetype.chunks.len(), 1);
        assert_eq!(archetype.chunks[0].layout.capacity(), 1);

        // The chunked accessors cover exactly the live rows, so publish `set_len(1)`
        // before writing through them. Both columns are trivial, so a byte copy into
        // each component-sized slot is safe; the key is a plain integer struct whose
        // slot needs no drop glue.
        unsafe {
            archetype.chunks[0].set_len(1);
        }
        archetype.chunks[0].get_entity_keys_mut()[0] =
            EntityKey { index: 7, generation: 3, instance_id: 9 };
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

        // The relocated data survived the move.
        let keys = archetype.chunks[0].get_entity_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].index, 7);
        assert_eq!(keys[0].generation, 3);
        assert_eq!(keys[0].instance_id, 9);

        // Each column reads back through `get_column_chunks` as one component-sized slot.
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
        // A single 8000-byte column: per-entity = 8000 + entity key, so the maximum
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
        archetype.chunks[0].get_entity_keys_mut()[0] =
            EntityKey { index: 0, generation: 0, instance_id: 0 };
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
}
