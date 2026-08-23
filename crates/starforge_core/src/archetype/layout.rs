use thiserror::Error;

use crate::archetype::ArchetypeMeta;

/// The byte layout of an archetype's chunk buffer.
/// The buffer is a SOA (structure-of-arrays) containing component columns data.
pub struct ArchetypeChunkLayout {
    capacity: usize,
    buffer_size: usize,
    buffer_align: usize,
    column_offsets: Vec<usize>,
}

impl ArchetypeChunkLayout {
    /// 16 KB
    pub const MAX_BUFFER_SIZE_BYTE: usize = 16 * 1024;

    /// Number of entities (and elements per column array) the buffer can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total size of the buffer in bytes, including any padding.
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    /// Alignment of the buffer allocation: the first (largest-aligned) column's alignment,
    /// or 1 when the archetype has no columns.
    pub fn buffer_align(&self) -> usize {
        self.buffer_align
    }

    /// Byte offset of each column array from the start of the buffer, in `columns` order.
    pub fn column_offsets(&self) -> &[usize] {
        &self.column_offsets
    }

    /// Builds a layout sized to hold exactly `capacity` entities.
    ///
    /// Columns are packed at the head of the buffer. The total byte size is rounded up to
    /// `buffer_align`, so any slack appears at the tail after the last column array.
    ///
    /// # Errors
    ///
    /// Returns [`ArchetypeChunkLayoutError::BufferTooLarge`] if the computed buffer size
    /// exceeds [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    pub fn with_capacity(meta: &ArchetypeMeta, capacity: usize) -> Result<Self, Error> {
        let mut columns_size = 0;
        let mut column_offsets = Vec::with_capacity(meta.columns().len());
        for column in meta.columns() {
            column_offsets.push(columns_size);
            columns_size += column.stride * capacity;
        }

        let buffer_align = Self::buffer_align_for(meta);
        let buffer_size = columns_size.next_multiple_of(buffer_align);

        if buffer_size > Self::MAX_BUFFER_SIZE_BYTE {
            return Err(Error::BufferTooLarge { buffer_size, max: Self::MAX_BUFFER_SIZE_BYTE });
        }

        Ok(Self { capacity, buffer_size, buffer_align, column_offsets })
    }

    /// Builds a layout fitting within a `buffer_size`-byte budget, holding the largest
    /// capacity that fits.
    ///
    /// The recorded [`Self::buffer_size`] keeps the requested `buffer_size` exactly. For the
    /// layout itself the budget is rounded down to the buffer's alignment. If the archetype
    /// has no columns (`per_entity == 0`), capacity is treated as unbounded and reported as
    /// `usize::MAX`.
    ///
    /// # Errors
    ///
    /// Returns [`ArchetypeChunkLayoutError::BufferTooLarge`] if `buffer_size` exceeds
    /// [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    pub fn with_buffer_size(meta: &ArchetypeMeta, buffer_size: usize) -> Result<Self, Error> {
        if buffer_size > Self::MAX_BUFFER_SIZE_BYTE {
            return Err(Error::BufferTooLarge { buffer_size, max: Self::MAX_BUFFER_SIZE_BYTE });
        }

        // Round down to a buffer-aligned budget and fit as many rows as possible.
        let buffer_align = Self::buffer_align_for(meta);
        let aligned_size = buffer_size - buffer_size % buffer_align;
        let per_entity = meta.columns().iter().map(|c| c.stride).sum::<usize>();
        let capacity = if per_entity == 0 {
            usize::MAX
        } else {
            aligned_size / per_entity
        };

        // Columns are tightly packed at the head; slack (if any) remains at the tail.
        let mut columns_size = 0;
        let mut column_offsets = Vec::with_capacity(meta.columns().len());
        for column in meta.columns() {
            column_offsets.push(columns_size);
            columns_size += column.stride * capacity;
        }

        Ok(Self { capacity, buffer_size, buffer_align, column_offsets })
    }

    /// Alignment of the buffer allocation: the first column's alignment, or 1 when empty.
    fn buffer_align_for(meta: &ArchetypeMeta) -> usize {
        meta.columns().first().map(|entry| entry.align).unwrap_or(1)
    }
}

/// Errors returned when building an [`ArchetypeChunkLayout`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// The computed buffer size exceeds [`ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE`].
    #[error("buffer size {buffer_size} exceeds the maximum of {max} bytes")]
    BufferTooLarge { buffer_size: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use crate::{
        archetype::meta::ColumnEntry,
        component::{ComponentKind, ComponentMeta, ComponentRegistry},
        types::{TypeId, TypeMeta, TypeRegistry},
    };

    /// Script columns registered by [`TestContext::mock`], in registration order (component
    /// key index 0 and 1). `ArchetypeMeta::new` reorders them by alignment descending, so
    /// column 0 (align 8) comes first and column 1 (align 4) second.
    fn columns() -> [TypeMeta; 2] {
        [
            TypeMeta::new(TypeId::of_script(1), 8, 8, TypeName::of_script("column_0")),
            TypeMeta::new(TypeId::of_script(2), 4, 4, TypeName::of_script("column_1")),
        ]
    }
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

        pub fn column(&self, id: TypeId) -> ColumnEntry {
            let type_key = *self.type_reg.id_to_key(&id).unwrap();
            let comp_key = *self.comp_reg.id_to_key(&id).unwrap();
            ColumnEntry::new(type_key, comp_key, &self.type_reg, &self.comp_reg).unwrap()
        }

        /// Builds an `ArchetypeMeta` over [`COLUMNS`], in registration order.
        pub fn meta(&self) -> ArchetypeMeta {
            ArchetypeMeta::new(columns().iter().map(|c| self.column(c.id)).collect())
        }
    }

    #[test]
    fn lays_out_columns_without_gaps() {
        let ctx = TestContext::mock();
        let layout = ArchetypeChunkLayout::with_capacity(&ctx.meta(), 10).unwrap();

        // column 0 (align 8) sorts before column 1 (align 4), so its array starts at offset 0.
        assert_eq!(layout.capacity(), 10);
        assert_eq!(layout.column_offsets(), vec![0, columns()[0].size * 10]);
        assert_eq!(layout.buffer_size(), columns()[0].size * 10 + columns()[1].size * 10);
        assert_eq!(layout.buffer_align(), columns()[0].align);
    }

    #[test]
    fn excessive_capacity_is_rejected() {
        let ctx = TestContext::mock();
        // Per-entity bytes are 8 + 4; 2000 rows exceed the 16 KiB cap.
        let result = ArchetypeChunkLayout::with_capacity(&ctx.meta(), 2000);

        assert!(matches!(
            result,
            Err(Error::BufferTooLarge {
                buffer_size,
                max: ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE,
            }) if buffer_size > ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE
        ));
    }

    #[test]
    fn empty_meta_has_empty_column_layout() {
        let meta = ArchetypeMeta::new(vec![]);
        let capacity_layout = ArchetypeChunkLayout::with_capacity(&meta, 100).unwrap();
        let size_layout = ArchetypeChunkLayout::with_buffer_size(&meta, 1024).unwrap();

        assert_eq!(capacity_layout.column_offsets(), vec![]);
        assert_eq!(capacity_layout.buffer_align(), 1);
        assert_eq!(capacity_layout.buffer_size(), 0);
        assert_eq!(size_layout.capacity(), usize::MAX);
        assert_eq!(size_layout.buffer_size(), 1024);
    }

    #[test]
    fn buffer_size_preserves_requested_budget() {
        let ctx = TestContext::mock();
        // The recorded buffer size keeps the requested budget; only the layout computation
        // rounds down to the 8-aligned boundary, leaving the 4 trailing bytes unused.
        let layout = ArchetypeChunkLayout::with_buffer_size(&ctx.meta(), 4096 + 4).unwrap();

        assert_eq!(layout.buffer_size(), 4096 + 4);
        assert_eq!(layout.capacity(), 4096 / (columns()[0].size + columns()[1].size));

        // Used payload is column bytes only and stays within the aligned budget.
        let used_bytes = (columns()[0].size + columns()[1].size) * layout.capacity();
        assert!(used_bytes <= 4096);
    }

    #[test]
    fn buffer_size_over_max_is_rejected() {
        let ctx = TestContext::mock();
        let result = ArchetypeChunkLayout::with_buffer_size(
            &ctx.meta(),
            ArchetypeChunkLayout::MAX_BUFFER_SIZE_BYTE + 1,
        );

        assert!(matches!(result, Err(Error::BufferTooLarge { .. })));
    }
}
