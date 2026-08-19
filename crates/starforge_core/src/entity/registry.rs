pub struct EntityRegistry {
    // TODO
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityKey {
    pub index: usize,
    pub generation: u32,
    pub instance_id: u32,
}
