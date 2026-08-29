use crate::prelude::*;

use thiserror::Error;

/// Metadata describing a sparse set's stored component type, referenced both by its
/// [`TypeKey`] (for layout metadata) and its [`ComponentKey`] (for storage metadata).
///
/// The layout and component fields are **frozen snapshots** taken at [`SparseSetHeader::new`]
/// time: once the header is built, downstream code reads size/align/drop info straight from
/// these fields and never re-queries the (dynamically modifiable) registries.
pub struct SparseSetHeader {
    pub type_id: TypeId,
    pub type_key: TypeKey,
    pub comp_key: ComponentKey,
    /// The size of the component type in bytes.
    pub stride: usize,
    /// The alignment of the component type in bytes.
    pub align: usize,
    pub name: TypeName,
    pub comp_kind: ComponentKind,
}

impl SparseSetHeader {
    /// Builds a sparse set header by resolving `type_key`/`comp_key` against the registries.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Type`] or [`Error::Component`] if either key no longer resolves, and
    /// [`Error::KeyMismatch`] if the two keys resolve to different underlying types.
    pub fn new(
        type_key: TypeKey,
        comp_key: ComponentKey,
        type_registry: &TypeRegistry,
        comp_registry: &ComponentRegistry,
    ) -> Result<Self, Error> {
        let type_meta = type_registry.key_to_meta(&type_key)?;
        let comp_meta = comp_registry.key_to_meta(&comp_key)?;
        if type_meta.id != comp_meta.id {
            return Err(Error::KeyMismatch { type_id: type_meta.id, comp_id: comp_meta.id });
        }
        Ok(Self {
            type_id: type_meta.id,
            type_key,
            comp_key,
            stride: type_meta.size,
            align: type_meta.align,
            name: type_meta.name.clone(),
            comp_kind: comp_meta.kind,
        })
    }
}

/// Errors returned when building a [`SparseSetHeader`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A [`TypeKey`] does not resolve in the [`TypeRegistry`].
    #[error(transparent)]
    Type(#[from] crate::types::Error),

    /// A [`ComponentKey`] does not resolve in the [`ComponentRegistry`].
    #[error(transparent)]
    Component(#[from] crate::component::Error),

    /// The type key and component key resolve to different underlying types.
    #[error("type key resolves to {type_id:?} but component key resolves to {comp_id:?}")]
    KeyMismatch { type_id: TypeId, comp_id: TypeId },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{ComponentKind, ComponentMeta};
    use crate::types::TypeMeta;

    struct TestContext {
        type_reg: TypeRegistry,
        comp_reg: ComponentRegistry,
    }

    impl TestContext {
        /// Builds a context containing two registered types, `script(1)` and `script(2)`.
        pub fn mock() -> Self {
            let mut type_reg = TypeRegistry::default();
            let mut comp_reg = ComponentRegistry::default();

            for id in [TypeId::of_script(1), TypeId::of_script(2)] {
                type_reg.register(TypeMeta::new(id, 8, 8, TypeName::of_script("test")));
                comp_reg.register(ComponentMeta::new(id, ComponentKind::Trivial));
            }

            Self { type_reg, comp_reg }
        }

        pub fn header(&self, id: TypeId) -> SparseSetHeader {
            let type_key = *self.type_reg.id_to_key(&id).unwrap();
            let comp_key = *self.comp_reg.id_to_key(&id).unwrap();
            SparseSetHeader::new(type_key, comp_key, &self.type_reg, &self.comp_reg).unwrap()
        }
    }

    #[test]
    fn rejects_mismatched_type_and_component_keys() {
        let ctx = TestContext::mock();
        let type_key = *ctx.type_reg.id_to_key(&TypeId::of_script(1)).unwrap();
        let comp_key = *ctx.comp_reg.id_to_key(&TypeId::of_script(2)).unwrap();

        let result = SparseSetHeader::new(type_key, comp_key, &ctx.type_reg, &ctx.comp_reg);

        assert!(matches!(
            result,
            Err(Error::KeyMismatch { type_id, comp_id })
                if type_id == TypeId::of_script(1) && comp_id == TypeId::of_script(2)
        ));
    }

    #[test]
    fn rejects_unresolved_type_key() {
        let mut ctx = TestContext::mock();
        let header = ctx.header(TypeId::of_script(1));
        // Stale the type key so it no longer resolves.
        ctx.type_reg.unregister(header.type_key).unwrap();

        let result =
            SparseSetHeader::new(header.type_key, header.comp_key, &ctx.type_reg, &ctx.comp_reg);

        assert!(matches!(result, Err(Error::Type(_))));
    }

    #[test]
    fn rejects_unresolved_component_key() {
        let mut ctx = TestContext::mock();
        let header = ctx.header(TypeId::of_script(1));
        // Stale the component key so it no longer resolves.
        ctx.comp_reg.unregister(header.comp_key).unwrap();

        let result =
            SparseSetHeader::new(header.type_key, header.comp_key, &ctx.type_reg, &ctx.comp_reg);

        assert!(matches!(result, Err(Error::Component(_))));
    }
}
