use crate::prelude::*;

mod registry;

pub struct SparseSetHeader {
    pub type_id: TypeId,
    pub type_key: TypeKey,
    pub comp_key: ComponentKey,
    pub stride: usize,
    pub align: usize,
    pub name: TypeName,
    pub comp_kind: ComponentKind,
}

pub struct SparseSet {
    pub header: SparseSetHeader,
}
