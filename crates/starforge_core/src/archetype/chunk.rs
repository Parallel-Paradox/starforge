use std::{
    alloc::{Layout, dealloc},
    ptr::NonNull,
    slice::{ChunksExact, ChunksExactMut},
    sync::Arc,
};

use crate::{
    archetype::{ArchetypeChunkLayout, ArchetypeMeta},
    prelude::*,
};

pub struct ArchetypeChunk {
    /// The metadata describing the archetype of this chunk, including its columns and their types.
    pub meta: Arc<ArchetypeMeta>,
    /// The layout of the chunk's buffer, including offsets and sizes of component columns.
    pub layout: Arc<ArchetypeChunkLayout>,
    buf_ptr: NonNull<u8>,
    len: usize,
}

unsafe impl Send for ArchetypeChunk {}

impl ArchetypeChunk {
    /// Creates a new `ArchetypeChunk` with the given metadata and layout,
    /// allocating a buffer for its data.
    pub fn new(meta: Arc<ArchetypeMeta>, layout: Arc<ArchetypeChunkLayout>) -> Self {
        Self::assert_meta_matches_layout(&meta, &layout);
        let buf_ptr = if layout.buffer_size() == 0 {
            NonNull::<u8>::dangling()
        } else {
            let allocation_layout =
                Layout::from_size_align(layout.buffer_size(), layout.buffer_align())
                    .expect("archetype chunk layout must be valid");
            NonNull::new(unsafe { std::alloc::alloc(allocation_layout) })
                .unwrap_or_else(|| std::alloc::handle_alloc_error(allocation_layout))
        };

        Self { meta, layout, buf_ptr, len: 0 }
    }

    fn drop_live_components(&mut self) {
        if self.len == 0 {
            return;
        }

        for (index, column) in self.meta.columns().iter().enumerate() {
            let ComponentKind::NonTrivial { drop_fn } = column.comp_kind else {
                continue;
            };

            unsafe {
                let elements = self.buf_ptr.as_ptr().add(self.layout.column_offsets()[index]);
                for i in 0..self.len {
                    drop_fn(elements.add(i * column.stride));
                }
            }
        }
        self.len = 0;
    }

    fn deallocate_buffer(&mut self) {
        if self.layout.buffer_size() == 0 {
            return;
        }

        let layout = Layout::from_size_align(self.layout.buffer_size(), self.layout.buffer_align())
            .expect("archetype chunk layout must be valid");
        unsafe {
            dealloc(self.buf_ptr.as_ptr(), layout);
        }
    }
}

impl Drop for ArchetypeChunk {
    fn drop(&mut self) {
        self.drop_live_components();
        self.deallocate_buffer();
    }
}

impl ArchetypeChunk {
    /// Returns the number of valid entities in this chunk.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Sets the number of valid entities in this chunk.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `len` does not exceed the chunk's capacity and
    /// the elements in the range `[self.len, len)` are properly initialized.
    pub unsafe fn set_len(&mut self, len: usize) {
        debug_assert!(len <= self.layout.capacity());
        self.len = len;
    }

    /// Returns the bytes of the `index`-th column covering the valid entities: the first
    /// `len` elements of that column array.
    ///
    /// The slice has length `self.len() * element_size`; slots beyond `len` (up to
    /// `capacity`) hold no valid entity. The bytes are untyped; reinterpret them as the
    /// column's component type.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds of the archetype's columns.
    pub fn get_column(&self, index: usize) -> &[u8] {
        let element_size = self.meta.columns()[index].stride;
        let offset = self.layout.column_offsets()[index];
        // SAFETY: the `index`-th column array starts at `offset` and holds `capacity`
        // elements of `element_size` bytes; `len <= capacity` keeps the
        // `len * element_size`-byte slice within the buffer.
        unsafe {
            let ptr = self.buf_ptr.as_ptr().add(offset);
            std::slice::from_raw_parts(ptr, self.len * element_size)
        }
    }

    /// Returns the bytes of the `index`-th column covering the valid entities as a
    /// mutable slice: the first `len` elements of that column array.
    ///
    /// The slice has length `self.len() * element_size`; slots beyond `len` (up to
    /// `capacity`) hold no valid entity. To insert new values, place them with
    /// [`Self::get_column_mut_ptr`] and advance `self.len` via [`Self::set_len`].
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds of the archetype's columns.
    pub fn get_column_mut(&mut self, index: usize) -> &mut [u8] {
        let element_size = self.meta.columns()[index].stride;
        let offset = self.layout.column_offsets()[index];
        // SAFETY: the `index`-th column array starts at `offset` and holds `capacity`
        // elements of `element_size` bytes; `len <= capacity` keeps the
        // `len * element_size`-byte slice within the buffer.
        unsafe {
            let ptr = self.buf_ptr.as_ptr().add(offset);
            std::slice::from_raw_parts_mut(ptr, self.len * element_size)
        }
    }

    /// Returns an iterator over the `index`-th column's valid entities, one
    /// component-sized byte slice per entity.
    ///
    /// This is the lazy, allocation-free way to view the column as per-entity
    /// slices: each yielded `&[u8]` has length `element_size`, covering exactly one
    /// component value.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds of the archetype's columns.
    pub fn get_column_chunks(&self, index: usize) -> ChunksExact<'_, u8> {
        self.get_column(index).chunks_exact(self.meta.columns()[index].stride)
    }

    /// Returns a mutable iterator over the `index`-th column's valid entities, one
    /// component-sized byte slice per entity.
    ///
    /// This is the lazy, allocation-free way to view the column as per-entity
    /// slices: each yielded `&mut [u8]` has length `element_size`, covering exactly
    /// one component value.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds of the archetype's columns.
    pub fn get_column_chunks_mut(&mut self, index: usize) -> ChunksExactMut<'_, u8> {
        let element_size = self.meta.columns()[index].stride;
        self.get_column_mut(index).chunks_exact_mut(element_size)
    }

    /// Returns a non-null pointer to the start of the `index`-th column array, for placing
    /// new component values.
    ///
    /// # Safety
    ///
    /// The caller must ensure no aliasing references exist and must not read or
    /// overwrite slots beyond `self.len` by assignment: they are uninitialized. Convert the
    /// pointer with [`NonNull::as_ptr`] for raw pointer operations (write or copy), and advance
    /// `self.len` via [`Self::set_len`] so the safe getters see them.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds of the archetype's columns.
    pub unsafe fn get_column_mut_ptr(&mut self, index: usize) -> NonNull<u8> {
        // SAFETY: `buf_ptr` is a `buffer_size`-byte, `buffer_align`-aligned allocation
        // and the `index`-th column array of `capacity` elements fits entirely inside it
        // at its column offset.
        unsafe {
            NonNull::new_unchecked(self.buf_ptr.as_ptr().add(self.layout.column_offsets()[index]))
        }
    }

    /// Moves the live data of this chunk into `other`, the `len` of `self` will be reset.
    ///
    /// # Safety
    ///
    /// `self` and `other` must share the same `meta`, have enough capacity and distinct
    /// allocations.
    pub unsafe fn move_data_into(&mut self, other: &mut Self) {
        debug_assert!(Arc::ptr_eq(&self.meta, &other.meta), "chunks must share meta");
        debug_assert!(
            other.layout.capacity() >= self.len,
            "target chunk is smaller than the source's live count"
        );

        for (index, column) in self.meta.columns().iter().enumerate() {
            let bytes = self.len * column.stride;
            // SAFETY: both pointers are the starts of the `index`-th column array in
            // their own allocations (each chunk's offset is scaled by its own
            // capacity), and `self.len <= other.layout.capacity()` keeps the copy
            // within the destination column array.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buf_ptr.as_ptr().add(self.layout.column_offsets()[index]),
                    other.buf_ptr.as_ptr().add(other.layout.column_offsets()[index]),
                    bytes,
                );
            }
        }
        other.len = self.len;
        self.len = 0;
    }

    /// Debug-only invariant check: panics if `layout` is inconsistent with `meta`, i.e. could
    /// not have been built from it. Expands to nothing in release builds.
    fn assert_meta_matches_layout(meta: &ArchetypeMeta, layout: &ArchetypeChunkLayout) {
        debug_assert_eq!(
            meta.columns().len(),
            layout.column_offsets().len(),
            "meta and layout disagree on the column count"
        );

        // Each column offset must be the cumulative size of the columns before it.
        let mut columns_size = 0;
        for (offset, column) in layout.column_offsets().iter().zip(meta.columns()) {
            debug_assert_eq!(
                *offset, columns_size,
                "meta and layout disagree on column offset {offset}"
            );
            columns_size += column.stride * layout.capacity();
        }

        // Buffer alignment follows the first (largest-aligned) column, or 1 when empty.
        let expected_align = meta.columns().first().map(|column| column.align).unwrap_or(1);
        debug_assert_eq!(
            layout.buffer_align(),
            expected_align,
            "meta and layout disagree on the buffer alignment"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{archetype::ColumnEntry, macros::Component};

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
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

    /// A non-trivial component carrying a payload; dropping it records the payload on
    /// the [`Tracker`] it holds a handle to.
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

    fn columns() -> [TypeMeta; 2] {
        [
            TypeMeta::new(TypeId::of_script(1), 8, 8, TypeName::of_script("column_0")),
            TypeMeta::new(TypeId::of_script(2), 4, 4, TypeName::of_script("column_1")),
        ]
    }

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

            for column in columns() {
                type_reg.register(column.clone());
                comp_reg.register(ComponentMeta::new(column.id, ComponentKind::Trivial));
            }

            Self { type_reg, comp_reg }
        }

        /// Builds a context for the tracker tests: a non-trivial `Tracked` (16 bytes,
        /// 8-align) and a trivial `u64` (8 bytes, 8-align).
        pub fn mock_tracked() -> Self {
            let mut type_reg = TypeRegistry::default();
            let mut comp_reg = ComponentRegistry::default();
            type_reg.register(TypeMeta::of::<Tracked>());
            let trivial = TypeMeta::new(TypeId::of_script(1), 8, 8, TypeName::of_script("trivial"));
            type_reg.register(trivial);
            comp_reg.register(ComponentMeta::of::<Tracked>());
            comp_reg.register(ComponentMeta::new(TypeId::of_script(1), ComponentKind::Trivial));
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
                columns().iter().map(|c| self.column(c.id)).collect(),
                &self.type_reg,
                &self.comp_reg,
            )
            .unwrap()
        }

        /// Builds the two-column meta used by the tracker tests over this context's
        /// registrations.
        pub fn tracked_meta(&self) -> Arc<ArchetypeMeta> {
            let tracked_id = TypeId::of::<Tracked>();
            let trivial_id = TypeId::of_script(1);
            Arc::new(
                ArchetypeMeta::new(
                    Vec::from([tracked_id, trivial_id].map(|id| self.column(id))),
                    &self.type_reg,
                    &self.comp_reg,
                )
                .unwrap(),
            )
        }

        /// Builds a chunk over `meta`.
        pub fn chunk(&self, meta: ArchetypeMeta, capacity: usize) -> ArchetypeChunk {
            let layout = ArchetypeChunkLayout::with_capacity(&meta, capacity).unwrap();
            ArchetypeChunk::new(Arc::new(meta), Arc::new(layout))
        }

        /// Builds a chunk sharing `meta` with others, as `move_data_into` requires the two
        /// chunks to hold the same `Arc<ArchetypeMeta>`.
        pub fn chunk_shared(&self, meta: &Arc<ArchetypeMeta>, capacity: usize) -> ArchetypeChunk {
            let layout = ArchetypeChunkLayout::with_capacity(meta, capacity).unwrap();
            ArchetypeChunk::new(meta.clone(), Arc::new(layout))
        }
    }

    /// Places distinct patterns into all columns via the raw-pointer getters (covering
    /// the full capacity), then reads them back through the safe getters. Distinct
    /// patterns mean any offset error or region overlap corrupts a read-back.
    #[test]
    fn round_trip_distinct_patterns() {
        const CAPACITY: usize = 16;
        const LEN: usize = 7;

        let ctx = TestContext::mock();
        let mut chunk = ctx.chunk(ctx.meta(), CAPACITY);

        // Placement via the raw-pointer getters. The slots are uninitialized, hence
        // `ptr::write` rather than assignment.
        unsafe {
            let column_0 = chunk.get_column_mut_ptr(0).as_ptr();
            for i in 0..CAPACITY * 8 {
                std::ptr::write(column_0.add(i), (i * 7) as u8);
            }

            let column_1 = chunk.get_column_mut_ptr(1).as_ptr();
            for i in 0..CAPACITY * 4 {
                std::ptr::write(column_1.add(i), (i * 13) as u8);
            }
            chunk.set_len(LEN);
        }

        // Safe getters expose only the first `len` elements, matching the writes.
        let column_0 = chunk.get_column(0);
        assert_eq!(column_0.len(), LEN * 8);
        for (i, byte) in column_0.iter().enumerate() {
            assert_eq!(*byte, (i * 7) as u8);
        }

        assert_eq!(chunk.get_column_mut(0).len(), LEN * 8);

        let column_1 = chunk.get_column(1);
        assert_eq!(column_1.len(), LEN * 4);
        for (i, byte) in column_1.iter().enumerate() {
            assert_eq!(*byte, (i * 13) as u8);
        }
    }

    /// An empty-metadata chunk has no column views but can still track logical length.
    #[test]
    fn empty_meta_tracks_len_without_columns() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(vec![], &ctx.type_reg, &ctx.comp_reg).unwrap();
        let mut chunk = ctx.chunk(meta, 8);

        unsafe { chunk.set_len(3) };
        assert_eq!(chunk.len(), 3);
    }

    #[test]
    fn zero_capacity_chunk_has_empty_views() {
        let ctx = TestContext::mock();
        let chunk = ctx.chunk(ctx.meta(), 0);

        assert!(chunk.get_column(0).is_empty());
    }

    /// Out-of-bounds column access panics, as documented on the getters.
    #[test]
    #[should_panic(expected = "out of bounds")]
    fn get_column_out_of_bounds_panics() {
        let ctx = TestContext::mock();
        let chunk = ctx.chunk(ctx.meta(), 8);
        let _ = chunk.get_column(2); // only columns 0 and 1 exist
    }

    /// `set_len` beyond capacity panics via its debug assertion.
    #[test]
    #[should_panic]
    fn set_len_beyond_capacity_panics() {
        let ctx = TestContext::mock();
        let mut chunk = ctx.chunk(ctx.meta(), 8);
        unsafe { chunk.set_len(9) };
    }

    /// Dropping a chunk runs each non-trivial column's drop glue exactly once per live
    /// element — `len` times, not `capacity` — with pointers into the correct elements.
    #[test]
    fn drop_calls_non_trivial_columns_per_element() {
        let tracker = Tracker::new();
        let ctx = TestContext::mock_tracked();
        let meta = ctx.tracked_meta();
        let mut chunk = ctx.chunk_shared(&meta, 8);

        // Place four live `Tracked` values into the non-trivial column (column 0) via
        // `ptr::write` (not assignment): the slots are uninitialized, and assignment
        // would first drop garbage — the same trap a real ECS insertion avoids.
        unsafe {
            let elements = chunk.get_column_mut_ptr(0).as_ptr().cast::<Tracked>();
            for i in 0..4 {
                std::ptr::write(
                    elements.add(i),
                    Tracked { value: (i as u64 + 1) * 10, tracker: tracker.clone() },
                );
            }
            chunk.set_len(4)
        }

        drop(chunk);

        assert_eq!(tracker.count.load(Ordering::SeqCst), 4);
        assert_eq!(tracker.sum.load(Ordering::SeqCst), 10 + 20 + 30 + 40);
    }

    /// `move_data_into` relocates every column's live elements into the target chunk,
    /// resets the source's length, and hands the non-trivial
    /// components over without dropping them twice: the source is emptied before it is
    /// dropped, so each component is dropped exactly once, from the target.
    #[test]
    fn move_data_into_transfers_and_drops_once() {
        let tracker = Tracker::new();
        let ctx = TestContext::mock_tracked();
        let meta = ctx.tracked_meta();
        let mut source = ctx.chunk_shared(&meta, 8);
        let mut target = ctx.chunk_shared(&meta, 16);

        // SAFETY: writes only into the source's slots.
        unsafe {
            // The chunked-mut getters expose exactly the first `len` rows, so the rows
            // must be declared live before they can be filled through
            // `get_column_chunks_mut`.
            source.set_len(3);

            // Column 0 is non-trivial: `ptr::write` (not assignment) moves each value
            // into its uninitialized slot without dropping garbage.
            for (i, slot) in source.get_column_chunks_mut(0).enumerate() {
                std::ptr::write(
                    slot.as_mut_ptr().cast::<Tracked>(),
                    Tracked { value: (i as u64 + 1) * 10, tracker: tracker.clone() },
                );
            }

            // Column 1 is a trivial `u64`: a plain byte copy into the uninitialized
            // slot.
            for (i, slot) in source.get_column_chunks_mut(1).enumerate() {
                slot.copy_from_slice(&((i as u64 + 1) << 32).to_ne_bytes());
            }

            source.move_data_into(&mut target)
        };

        // The source is emptied; the target takes over the moved count.
        assert_eq!(source.len(), 0);
        assert_eq!(target.len(), 3);

        // Read the non-trivial column back through `get_column_chunks`: one
        // component-sized slot per row.
        for (i, slot) in target.get_column_chunks(0).enumerate() {
            // SAFETY: the slot is the moved `Tracked` value of the `i`-th row.
            let tracked = unsafe { &*slot.as_ptr().cast::<Tracked>() };
            assert_eq!(tracked.value, (i as u64 + 1) * 10);
        }

        // Read the trivial column back; each slot is exactly `size_of::<u64>()` bytes.
        let values: Vec<u64> = target
            .get_column_chunks(1)
            .map(|slot| u64::from_ne_bytes(slot.try_into().unwrap()))
            .collect();
        assert_eq!(values, vec![1u64 << 32, 2 << 32, 3 << 32]);

        drop(source);
        // The emptied source drops nothing.
        assert_eq!(tracker.count.load(Ordering::SeqCst), 0);

        drop(target);
        // Each component dropped exactly once, from the target (the source's reset
        // length prevented a second drop from the old buffer).
        assert_eq!(tracker.count.load(Ordering::SeqCst), 3);
        assert_eq!(tracker.sum.load(Ordering::SeqCst), 10 + 20 + 30);
    }
}
