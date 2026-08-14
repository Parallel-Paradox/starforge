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
}

impl ArchetypeChunk {
    pub fn new(meta: Rc<ArchetypeMeta>, layout: Rc<ArchetypeChunkLayout>) -> Self {
        Self::assert_meta_matches_layout(&meta, &layout);
        let buf_ptr = unsafe {
            let layout =
                Layout::from_size_align_unchecked(layout.buffer_size(), layout.buffer_align());
            std::alloc::alloc(layout)
        };

        Self { meta, layout, buf_ptr }
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
