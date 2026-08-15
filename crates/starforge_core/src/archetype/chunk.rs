use std::{
    alloc::{Layout, dealloc},
    rc::Rc,
};

use crate::{
    archetype::{ArchetypeChunkLayout, ArchetypeMeta},
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

    /// Returns a mutable slice over the *entire* entity key array: `capacity` elements,
    /// **including slots beyond `self.len`** that hold no valid entity. Only the first
    /// `self.len` entries correspond to live entities.
    ///
    /// # Safety
    ///
    /// The caller must ensure no aliasing references exist and must not treat the slots
    /// beyond `self.len` as valid entities.
    pub unsafe fn get_entity_keys_mut(&mut self) -> &mut [EntityKey] {
        // SAFETY: `buf_ptr` is a `buffer_size`-byte, `buffer_align`-aligned allocation and
        // the array of `capacity` elements fits entirely inside it at `entity_key_offset`.
        unsafe {
            let keys = self.buf_ptr.add(self.layout.entity_key_offset()) as *mut EntityKey;
            std::slice::from_raw_parts_mut(keys, self.layout.capacity())
        }
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

    /// Returns a mutable byte slice over the *entire* `index`-th column array:
    /// `capacity` elements, **including slots beyond `self.len`** that hold no valid
    /// entity. Only the first `self.len` elements correspond to live entities.
    ///
    /// # Safety
    ///
    /// The caller must ensure no aliasing references exist and must not treat the slots
    /// beyond `self.len` as valid entities.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds of the archetype's columns.
    pub unsafe fn get_column_mut(&mut self, index: usize) -> &mut [u8] {
        let element_size = self.meta.columns()[index].type_meta.size;
        let offset = self.layout.column_offsets()[index];
        // SAFETY: `buf_ptr` is a `buffer_size`-byte, `buffer_align`-aligned allocation
        // and the `index`-th column array of `capacity` elements of `element_size` bytes
        // fits entirely inside it at `offset`.
        unsafe {
            let ptr = self.buf_ptr.add(offset);
            std::slice::from_raw_parts_mut(ptr, self.layout.capacity() * element_size)
        }
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
    use super::*;
    use crate::{
        archetype::ColumnEntry,
        component::{ComponentKind, ComponentMeta, ComponentRegistry},
        types::{TypeId, TypeMeta, TypeRegistry},
    };

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

    /// Writes distinct patterns into all regions via the mut getters, then reads each back
    /// through the safe getters.
    #[test]
    fn round_trip_distinct_patterns() {
        const CAPACITY: usize = 16;
        const LEN: usize = 7;

        let mut chunk = chunk(meta(), CAPACITY);
        unsafe { chunk.set_len(LEN) };

        // Mut getters cover the full capacity.
        unsafe {
            let keys = chunk.get_entity_keys_mut();
            assert_eq!(keys.len(), CAPACITY);
            for (i, key) in keys.iter_mut().enumerate() {
                *key = EntityKey { index: i, generation: i as u32, instance_id: 1 };
            }

            let column_0 = chunk.get_column_mut(0);
            assert_eq!(column_0.len(), CAPACITY * 8);
            for (i, byte) in column_0.iter_mut().enumerate() {
                *byte = (i * 7) as u8;
            }

            let column_1 = chunk.get_column_mut(1);
            assert_eq!(column_1.len(), CAPACITY * 4);
            for (i, byte) in column_1.iter_mut().enumerate() {
                *byte = (i * 13) as u8;
            }
        }

        // Safe getters expose only the first `len` elements, matching the writes.
        let keys = chunk.get_entity_keys();
        assert_eq!(keys.len(), LEN);
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(key.index, i);
            assert_eq!(key.generation, i as u32);
            assert_eq!(key.instance_id, 1);
        }

        let column_0 = chunk.get_column(0);
        assert_eq!(column_0.len(), LEN * 8);
        for (i, byte) in column_0.iter().enumerate() {
            assert_eq!(*byte, (i * 7) as u8);
        }

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
        unsafe { chunk.set_len(3) };

        unsafe {
            for (i, key) in chunk.get_entity_keys_mut().iter_mut().enumerate() {
                *key = EntityKey { index: i, generation: 0, instance_id: i as u32 };
            }
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
}
