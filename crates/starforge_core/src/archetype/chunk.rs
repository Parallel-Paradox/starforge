use std::{
    alloc::{Layout, dealloc},
    rc::Rc,
};

use crate::{
    archetype::{ArchetypeChunkLayout, ArchetypeMeta},
    component::ComponentKind,
    entity::EntityKey,
};

pub struct ArchetypeChunk {
    pub meta: Rc<ArchetypeMeta>,
    pub layout: Rc<ArchetypeChunkLayout>,
    buf_ptr: *mut u8,
    len: usize,
}

impl ArchetypeChunk {
    pub fn new(meta: Rc<ArchetypeMeta>, layout: Rc<ArchetypeChunkLayout>) -> Self {
        Self::assert_meta_matches_layout(&meta, &layout);
        let buf_ptr = unsafe {
            // SAFETY: `buffer_align` is a power of two (the max of the column alignments,
            // which `TypeMeta::new` checks, and the `EntityKey` alignment), and
            // `buffer_size` is bounded by `MAX_BUFFER_SIZE_BYTE`, so the layout is valid.
            let layout =
                Layout::from_size_align_unchecked(layout.buffer_size(), layout.buffer_align());
            std::alloc::alloc(layout)
        };

        Self { meta, layout, buf_ptr, len: 0 }
    }
}

impl Drop for ArchetypeChunk {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: Run each non-trivial column's drop glue over its live elements,
            // then free the buffer.
            for (index, column) in self.meta.columns().iter().enumerate() {
                if let ComponentKind::NonTrivial { drop_fn } = column.comp_meta.kind {
                    let elements = self.buf_ptr.add(self.layout.column_offsets()[index]);
                    for i in 0..self.len {
                        drop_fn(elements.add(i * column.type_meta.size));
                    }
                }
            }

            // SAFETY: `buf_ptr` is the live allocation made in `new` with this exact
            // layout, and `&mut self` guarantees no other reference to the buffer exists;
            // the layout is valid for the same reasons as in `new`.
            let layout = Layout::from_size_align_unchecked(
                self.layout.buffer_size(),
                self.layout.buffer_align(),
            );
            dealloc(self.buf_ptr, layout);
        }
    }
}

impl ArchetypeChunk {
    pub fn len(&self) -> usize {
        self.len
    }

    pub unsafe fn set_len(&mut self, len: usize) {
        debug_assert!(len <= self.layout.capacity());
        self.len = len;
    }

    /// Returns the valid entity keys: the first `len` entries of the entity key array.
    ///
    /// The slice has length `self.len()`; slots beyond `len` (up to `capacity`) hold no
    /// valid entity.
    pub fn get_entity_keys(&self) -> &[EntityKey] {
        // SAFETY: the array starts at `entity_key_offset` and holds `capacity` elements;
        // `len <= capacity` keeps the `len`-element slice within the buffer.
        unsafe {
            let keys = self.buf_ptr.add(self.layout.entity_key_offset()) as *const EntityKey;
            std::slice::from_raw_parts(keys, self.len)
        }
    }

    /// Returns the valid entity keys as a mutable slice: the first `len` entries of the
    /// entity key array.
    ///
    /// The slice has length `self.len()`; slots beyond `len` (up to `capacity`) hold no
    /// valid entity. To insert new keys, place them with [`Self::get_entity_keys_mut_ptr`]
    /// and advance `self.len` via [`Self::set_len`].
    pub fn get_entity_keys_mut(&mut self) -> &mut [EntityKey] {
        // SAFETY: the array starts at `entity_key_offset` and holds `capacity` elements;
        // `len <= capacity` keeps the `len`-element slice within the buffer.
        unsafe {
            let keys = self.buf_ptr.add(self.layout.entity_key_offset()) as *mut EntityKey;
            std::slice::from_raw_parts_mut(keys, self.len)
        }
    }

    /// Returns a raw pointer to the start of the entity key array, for placing new keys.
    ///
    /// # Safety
    ///
    /// The caller must ensure no aliasing references exist and must not read or
    /// overwrite slots beyond `self.len` by assignment: they are uninitialized. Write
    /// new keys with `ptr::write` and advance `self.len` via [`Self::set_len`] so the
    /// safe getters see them.
    pub unsafe fn get_entity_keys_mut_ptr(&mut self) -> *mut EntityKey {
        // SAFETY: `buf_ptr` is a `buffer_size`-byte, `buffer_align`-aligned allocation
        // and the array of `capacity` elements fits entirely inside it at `entity_key_offset`.
        unsafe { self.buf_ptr.add(self.layout.entity_key_offset()) as *mut EntityKey }
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
        let element_size = self.meta.columns()[index].type_meta.size;
        let offset = self.layout.column_offsets()[index];
        // SAFETY: the `index`-th column array starts at `offset` and holds `capacity`
        // elements of `element_size` bytes; `len <= capacity` keeps the
        // `len * element_size`-byte slice within the buffer.
        unsafe {
            let ptr = self.buf_ptr.add(offset);
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
        let element_size = self.meta.columns()[index].type_meta.size;
        let offset = self.layout.column_offsets()[index];
        // SAFETY: the `index`-th column array starts at `offset` and holds `capacity`
        // elements of `element_size` bytes; `len <= capacity` keeps the
        // `len * element_size`-byte slice within the buffer.
        unsafe {
            let ptr = self.buf_ptr.add(offset);
            std::slice::from_raw_parts_mut(ptr, self.len * element_size)
        }
    }

    /// Returns a raw pointer to the start of the `index`-th column array, for placing
    /// new component values.
    ///
    /// # Safety
    ///
    /// The caller must ensure no aliasing references exist and must not read or
    /// overwrite slots beyond `self.len` by assignment: they are uninitialized. Write
    /// new values with `ptr::write` and advance `self.len` via [`Self::set_len`] so the
    /// safe getters see them.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds of the archetype's columns.
    pub unsafe fn get_column_mut_ptr(&mut self, index: usize) -> *mut u8 {
        // SAFETY: `buf_ptr` is a `buffer_size`-byte, `buffer_align`-aligned allocation
        // and the `index`-th column array of `capacity` elements fits entirely inside it
        // at its column offset.
        unsafe { self.buf_ptr.add(self.layout.column_offsets()[index]) }
    }

    /// Debug-only invariant check: panics if `layout` is inconsistent with `meta`, i.e. could
    /// not have been built from it. Expands to nothing in release builds.
    fn assert_meta_matches_layout(meta: &ArchetypeMeta, layout: &ArchetypeChunkLayout) {
        let entity_key_align = std::mem::align_of::<EntityKey>();
        let entity_key_size = std::mem::size_of::<EntityKey>();

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
            columns_size += column.type_meta.size * layout.capacity();
        }

        // Buffer alignment is the maximum of the entity key and the first column's alignment.
        let expected_align = meta
            .columns()
            .first()
            .map(|column| column.type_meta.align.max(entity_key_align))
            .unwrap_or(entity_key_align);
        debug_assert_eq!(
            layout.buffer_align(),
            expected_align,
            "meta and layout disagree on the buffer alignment"
        );

        // Entity keys end at the alignment-rounded buffer size, clear of the columns.
        let keys_end = layout.entity_key_offset() + entity_key_size * layout.capacity();
        let aligned_size = layout.buffer_size() - layout.buffer_size() % layout.buffer_align();
        debug_assert_eq!(
            keys_end, aligned_size,
            "entity key array does not end at the aligned boundary"
        );
        debug_assert!(
            layout.entity_key_offset() >= columns_size,
            "entity key array overlaps the column data"
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::macros::Component;

    use super::*;
    use crate::{
        archetype::ColumnEntry,
        component::{ComponentKind, ComponentMeta, ComponentRegistry},
        types::{TypeId, TypeMeta, TypeRegistry},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A non-trivial component carrying a payload observable from its drop glue.
    #[derive(Component)]
    struct Tracked(u64);

    impl Drop for Tracked {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, Ordering::SeqCst);
            DROP_SUM.fetch_add(self.0 as usize, Ordering::SeqCst);
        }
    }

    /// Number of times a [`Tracked`] has been dropped.
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    /// Sum of payloads of dropped [`Tracked`]s, to verify element pointers.
    static DROP_SUM: AtomicUsize = AtomicUsize::new(0);

    const COLUMNS: [TypeMeta; 2] = [
        TypeMeta::new(TypeId::of_script(1), 8, 8, "column_0"),
        TypeMeta::new(TypeId::of_script(2), 4, 4, "column_1"),
    ];

    fn registries() -> (TypeRegistry, ComponentRegistry) {
        let mut type_reg = TypeRegistry::default();
        let mut comp_reg = ComponentRegistry::default();

        for column in &COLUMNS {
            type_reg.register(column.clone());
            comp_reg.register(ComponentMeta::new(column.id, ComponentKind::Trivial));
        }

        (type_reg, comp_reg)
    }

    fn column(type_reg: &TypeRegistry, comp_reg: &ComponentRegistry, id: TypeId) -> ColumnEntry {
        let type_key = *type_reg.id_to_key(&id).unwrap();
        let comp_key = *comp_reg.id_to_key(&id).unwrap();
        let type_meta = type_reg.key_to_meta(&type_key).unwrap().clone();
        let comp_meta = comp_reg.key_to_meta(&comp_key).unwrap().clone();
        ColumnEntry { type_key, type_meta, comp_key, comp_meta }
    }

    fn meta() -> ArchetypeMeta {
        let (type_reg, comp_reg) = registries();
        ArchetypeMeta::new(
            COLUMNS
                .iter()
                .map(|c| column(&type_reg, &comp_reg, c.id))
                .collect(),
            &type_reg,
            &comp_reg,
        )
        .unwrap()
    }

    fn chunk(meta: ArchetypeMeta, capacity: usize) -> ArchetypeChunk {
        let layout = ArchetypeChunkLayout::with_capacity(&meta, capacity).unwrap();
        ArchetypeChunk::new(Rc::new(meta), Rc::new(layout))
    }

    /// Places distinct patterns into all regions via the raw-pointer getters (covering
    /// the full capacity), then reads them back through the safe getters. Distinct
    /// patterns mean any offset error or region overlap corrupts a read-back.
    #[test]
    fn round_trip_distinct_patterns() {
        const CAPACITY: usize = 16;
        const LEN: usize = 7;

        let mut chunk = chunk(meta(), CAPACITY);

        // Placement via the raw-pointer getters. The slots are uninitialized, hence
        // `ptr::write` rather than assignment.
        unsafe {
            let keys = chunk.get_entity_keys_mut_ptr();
            for i in 0..CAPACITY {
                std::ptr::write(
                    keys.add(i),
                    EntityKey { index: i, generation: i as u32, instance_id: 1 },
                );
            }

            let column_0 = chunk.get_column_mut_ptr(0);
            for i in 0..CAPACITY * 8 {
                std::ptr::write(column_0.add(i), (i * 7) as u8);
            }

            let column_1 = chunk.get_column_mut_ptr(1);
            for i in 0..CAPACITY * 4 {
                std::ptr::write(column_1.add(i), (i * 13) as u8);
            }
            chunk.set_len(LEN);
        }

        // Safe getters expose only the first `len` elements, matching the writes.
        let keys = chunk.get_entity_keys();
        assert_eq!(keys.len(), LEN);
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(key.index, i);
            assert_eq!(key.generation, i as u32);
            assert_eq!(key.instance_id, 1);
        }

        // The safe mutable getters are writable and cover exactly the live elements.
        {
            let keys_mut = chunk.get_entity_keys_mut();
            assert_eq!(keys_mut.len(), LEN);
            keys_mut[0].instance_id = 2;
        }
        assert_eq!(chunk.get_entity_keys()[0].instance_id, 2);

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

    /// An empty-metadata chunk holds only the entity key array; keys round-trip normally.
    #[test]
    fn empty_meta_keys_round_trip() {
        let (type_reg, comp_reg) = registries();
        let meta = ArchetypeMeta::new(vec![], &type_reg, &comp_reg).unwrap();
        let mut chunk = chunk(meta, 8);

        // Place three keys, then publish them with `set_len`.
        unsafe {
            let keys = chunk.get_entity_keys_mut_ptr();
            for i in 0..3 {
                std::ptr::write(
                    keys.add(i),
                    EntityKey { index: i, generation: 0, instance_id: i as u32 },
                );
            }
            chunk.set_len(3);
        }

        let keys = chunk.get_entity_keys();
        assert_eq!(keys.len(), 3);
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(key.index, i);
            assert_eq!(key.generation, 0);
            assert_eq!(key.instance_id, i as u32);
        }
    }

    /// Out-of-bounds column access panics, as documented on the getters.
    #[test]
    #[should_panic(expected = "out of bounds")]
    fn get_column_out_of_bounds_panics() {
        let chunk = chunk(meta(), 8);
        let _ = chunk.get_column(2); // only columns 0 and 1 exist
    }

    /// `set_len` beyond capacity panics via its debug assertion.
    #[test]
    #[should_panic]
    fn set_len_beyond_capacity_panics() {
        let mut chunk = chunk(meta(), 8);
        unsafe { chunk.set_len(9) };
    }

    /// Dropping a chunk runs each non-trivial column's drop glue exactly once per live
    /// element — `len` times, not `capacity` — with pointers into the correct elements.
    #[test]
    fn drop_calls_non_trivial_columns_per_element() {
        DROP_COUNT.store(0, Ordering::SeqCst);
        DROP_SUM.store(0, Ordering::SeqCst);

        // `Tracked` implements `Drop`, so `ComponentMeta::of` registers it as non-trivial
        // via `needs_drop` — exactly like a real component. The two 8-byte/8-align columns
        // tie on layout, so `ArchetypeMeta` orders them by component key index: the
        // trivial one first (index 0), the tracked one second.
        let trivial_id = TypeId::of_script(1);
        let tracked_id = TypeId::of::<Tracked>();

        let mut type_reg = TypeRegistry::default();
        let mut comp_reg = ComponentRegistry::default();
        type_reg.register(TypeMeta::new(trivial_id, 8, 8, "trivial"));
        type_reg.register(TypeMeta::of::<Tracked>());
        comp_reg.register(ComponentMeta::new(trivial_id, ComponentKind::Trivial));
        comp_reg.register(ComponentMeta::of::<Tracked>());

        let meta = ArchetypeMeta::new(
            Vec::from([trivial_id, tracked_id].map(|id| column(&type_reg, &comp_reg, id))),
            &type_reg,
            &comp_reg,
        )
        .unwrap();
        let tracked_col = meta.column_index(&tracked_id).unwrap();

        let mut chunk = chunk(meta, 8);

        // Place four live `Tracked` values into the non-trivial column via the raw-pointer
        // getter. `ptr::write` (not assignment) is required because the slots are
        // uninitialized: assignment would first drop whatever garbage is there — the same
        // trap a real ECS insertion avoids.
        unsafe {
            let elements = chunk.get_column_mut_ptr(tracked_col) as *mut Tracked;
            for i in 0..4 {
                std::ptr::write(elements.add(i), Tracked((i as u64 + 1) * 10));
            }
            chunk.set_len(4)
        }

        drop(chunk);

        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 4);
        assert_eq!(DROP_SUM.load(Ordering::SeqCst), 10 + 20 + 30 + 40);
    }
}
