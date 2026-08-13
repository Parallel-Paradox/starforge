use crate::archetype::meta::ArchetypeMeta;

#[derive(Default)]
pub struct ArchetypeChunkLayout {
    capacity: usize,
    buffer_size: usize,
    buffer_align: usize,
    column_offsets: Vec<usize>,
    entity_key_offset: usize,
}

impl ArchetypeChunkLayout {
    pub fn reset_with_capacity(&mut self, meta: &ArchetypeMeta, capacity: usize) {
        unimplemented!()
    }

    pub fn reset_with_buffer_size(&mut self, meta: &ArchetypeMeta, buffer_size: usize) {
        unimplemented!()
    }
}
