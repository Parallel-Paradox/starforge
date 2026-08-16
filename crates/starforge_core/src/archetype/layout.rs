use thiserror::Error;

use crate::{archetype::ArchetypeMeta, entity::EntityKey};

/// The byte layout of an archetype's chunk buffer.
/// The buffer is a SOA (structure-of-arrays), the entity key array is placed at the tail.
pub struct ArchetypeChunkLayout {
    capacity: usize,
    buffer_size: usize,
    buffer_align: usize,
    column_offsets: Vec<usize>,
    entity_key_offset: usize,
}

impl ArchetypeChunkLayout {
    /// 16 KB
    pub const MAX_BUFFER_SIZE_BYTE: usize = 16 * 1024;

    /// Alignment required by the entity key array.
    const ENTITY_KEY_ALIGN: usize = std::mem::align_of::<EntityKey>();
    /// Size of a single entity key.
    const ENTITY_KEY_SIZE: usize = std::mem::size_of::<EntityKey>();

    /// Number of entities (and elements per column array) the buffer can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total size of the buffer in bytes, including any padding.
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    /// Alignment of the buffer allocation: the maximum of the entity key's alignment and the
    /// first (largest-aligned) column's alignment.
    pub fn buffer_align(&self) -> usize {
        self.buffer_align
    }

    /// Byte offset of each column array from the start of the buffer, in `columns` order.
    pub fn column_offsets(&self) -> &[usize] {
        &self.column_offsets
    }

    /// Byte offset of the entity key array from the start of the buffer.
    ///
    /// The array occupies the tail, ending at a `buffer_align`-aligned boundary; any padding
    /// needed for alignment sits before it, between the last column and the keys.
    pub fn entity_key_offset(&self) -> usize {
        self.entity_key_offset
    }

    /// Builds a layout sized to hold exactly `capacity` entities.
    ///
    /// Columns are packed at the head of the buffer; the entity key array occupies the tail
    /// and ends at the next `buffer_align`-aligned boundary, so the buffer size is that
    /// aligned boundary. Any padding needed for the entity key array lands between the last
    /// column and the entity keys — never inside a column array.
    ///
    /// # Errors
    ///
    /// Returns [`ArchetypeChunkLayoutError::BufferTooLarge`] if the computed buffer size
    /// exceeds [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    pub fn with_capacity(
        meta: &ArchetypeMeta,
        capacity: usize,
    ) -> Result<Self, ArchetypeChunkLayoutError> {
        let mut columns_size = 0;
        let mut column_offsets = Vec::with_capacity(meta.columns().len());
        for column in meta.columns() {
            column_offsets.push(columns_size);
            columns_size += column.type_meta.size * capacity;
        }

        let entity_key_size = Self::ENTITY_KEY_SIZE * capacity;
        let buffer_align = Self::buffer_align_for(meta);
        let buffer_size = (columns_size + entity_key_size).next_multiple_of(buffer_align);
        let entity_key_offset = buffer_size - entity_key_size;

        if buffer_size > Self::MAX_BUFFER_SIZE_BYTE {
            return Err(ArchetypeChunkLayoutError::BufferTooLarge {
                buffer_size,
                max: Self::MAX_BUFFER_SIZE_BYTE,
            });
        }

        Ok(Self { capacity, buffer_size, buffer_align, column_offsets, entity_key_offset })
    }

    /// Builds a layout fitting within a `buffer_size`-byte budget, holding the largest
    /// capacity that fits.
    ///
    /// The recorded [`Self::buffer_size`] keeps the requested `buffer_size` exactly. For the
    /// layout itself the budget is rounded down to the buffer's alignment and the entity
    /// key array ends at that aligned boundary.
    ///
    /// # Errors
    ///
    /// Returns [`ArchetypeChunkLayoutError::BufferTooLarge`] if `buffer_size` exceeds
    /// [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    pub fn with_buffer_size(
        meta: &ArchetypeMeta,
        buffer_size: usize,
    ) -> Result<Self, ArchetypeChunkLayoutError> {
        if buffer_size > Self::MAX_BUFFER_SIZE_BYTE {
            return Err(ArchetypeChunkLayoutError::BufferTooLarge {
                buffer_size,
                max: Self::MAX_BUFFER_SIZE_BYTE,
            });
        }

        // Entity keys end at the budget rounded down to the buffer's alignment, so the
        // region before them holds a whole number of per-entity bytes and the largest
        // fitting capacity is a plain division.
        let buffer_align = Self::buffer_align_for(meta);
        let aligned_size = buffer_size - buffer_size % buffer_align;
        let per_entity =
            meta.columns().iter().map(|c| c.type_meta.size).sum::<usize>() + Self::ENTITY_KEY_SIZE;
        let capacity = aligned_size / per_entity;

        // Columns at the head, entity keys at the tail; any slack sits between them.
        let mut columns_size = 0;
        let mut column_offsets = Vec::with_capacity(meta.columns().len());
        for column in meta.columns() {
            column_offsets.push(columns_size);
            columns_size += column.type_meta.size * capacity;
        }
        let entity_key_offset = aligned_size - Self::ENTITY_KEY_SIZE * capacity;

        Ok(Self { capacity, buffer_size, buffer_align, column_offsets, entity_key_offset })
    }

    /// Alignment of the buffer allocation: the entity key's alignment, raised to the first
    /// column's alignment when the archetype has columns.
    fn buffer_align_for(meta: &ArchetypeMeta) -> usize {
        meta.columns()
            .first()
            .map(|column| column.type_meta.align.max(Self::ENTITY_KEY_ALIGN))
            .unwrap_or(Self::ENTITY_KEY_ALIGN)
    }
}

/// Errors returned when building an [`ArchetypeChunkLayout`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArchetypeChunkLayoutError {
    /// The computed buffer size exceeds [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    #[error("buffer size {buffer_size} exceeds the maximum of {max} bytes")]
    BufferTooLarge { buffer_size: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        archetype::meta::ColumnEntry,
        component::{ComponentKind, ComponentMeta, ComponentRegistry},
        types::{TypeId, TypeMeta, TypeRegistry},
    };

    /// Script columns registered by [`TestContext::mock`], in registration order (component
    /// key index 0 and 1). `ArchetypeMeta::new` reorders them by alignment descending, so
    /// column 0 (align 8) comes first and column 1 (align 4) second.
    const COLUMNS: [TypeMeta; 2] = [
        TypeMeta::new(TypeId::of_script(1), 8, 8, "column_0"),
        TypeMeta::new(TypeId::of_script(2), 4, 4, "column_1"),
    ];
    /// Size and alignment of the entity key array element.
    const ENTITY_KEY_SIZE: usize = std::mem::size_of::<EntityKey>();
    const ENTITY_KEY_ALIGN: usize = std::mem::align_of::<EntityKey>();

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

        pub fn column(&self, id: TypeId) -> ColumnEntry {
            let type_key = *self.type_reg.id_to_key(&id).unwrap();
            let comp_key = *self.comp_reg.id_to_key(&id).unwrap();
            let type_meta = self.type_reg.key_to_meta(&type_key).unwrap().clone();
            let comp_meta = self.comp_reg.key_to_meta(&comp_key).unwrap().clone();
            ColumnEntry { type_key, type_meta, comp_key, comp_meta }
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
    }

    #[test]
    fn lays_out_columns_without_gaps() {
        let ctx = TestContext::mock();
        let layout = ArchetypeChunkLayout::with_capacity(&ctx.meta(), 10).unwrap();

        // column 0 (align 8) sorts before column 1 (align 4), so its array starts at offset 0.
        assert_eq!(layout.capacity(), 10);
        assert_eq!(layout.column_offsets(), vec![0, COLUMNS[0].size * 10]);
        assert_eq!(
            layout.buffer_size(),
            COLUMNS[0].size * 10 + COLUMNS[1].size * 10 + ENTITY_KEY_SIZE * 10
        );
        assert_eq!(layout.buffer_align(), COLUMNS[0].align);
    }

    #[test]
    fn excessive_capacity_is_rejected() {
        let ctx = TestContext::mock();
        let result = ArchetypeChunkLayout::with_capacity(&ctx.meta(), 1000);

        assert!(matches!(
            result,
            Err(ArchetypeChunkLayoutError::BufferTooLarge {
                buffer_size,
                max: ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE,
            }) if buffer_size > ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE
        ));
    }

    #[test]
    fn empty_meta_only_keep_entity_key() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(vec![], &ctx.type_reg, &ctx.comp_reg).unwrap();
        let capacity_layout = ArchetypeChunkLayout::with_capacity(&meta, 100).unwrap();
        let size_layout = ArchetypeChunkLayout::with_buffer_size(&meta, 1024).unwrap();

        assert_eq!(capacity_layout.column_offsets(), vec![]);
        assert_eq!(capacity_layout.buffer_align(), ENTITY_KEY_ALIGN);
        assert_eq!(capacity_layout.buffer_size(), ENTITY_KEY_SIZE * 100);
        assert_eq!(size_layout.capacity(), 1024 / ENTITY_KEY_SIZE);
        assert_eq!(size_layout.buffer_size(), 1024);
    }

    #[test]
    fn buffer_size_preserves_requested_budget() {
        let ctx = TestContext::mock();
        // The recorded buffer size keeps the requested budget; only the layout computation
        // rounds down to the 8-aligned boundary, leaving the 4 trailing bytes unused.
        let layout = ArchetypeChunkLayout::with_buffer_size(&ctx.meta(), 4096 + 4).unwrap();

        assert_eq!(layout.buffer_size(), 4096 + 4);
        assert_eq!(layout.capacity(), 4096 / (COLUMNS[0].size + COLUMNS[1].size + ENTITY_KEY_SIZE));
        // Entity keys end at the aligned boundary, past the columns, before the tail slack.
        assert_eq!(layout.entity_key_offset() + ENTITY_KEY_SIZE * layout.capacity(), 4096);
    }

    #[test]
    fn buffer_size_over_max_is_rejected() {
        let ctx = TestContext::mock();
        let result = ArchetypeChunkLayout::with_buffer_size(
            &ctx.meta(),
            ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE + 1,
        );

        assert!(matches!(result, Err(ArchetypeChunkLayoutError::BufferTooLarge { .. })));
    }
}
