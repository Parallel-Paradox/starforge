use crate::archetype::ArchetypeSignature;
use crate::prelude::*;

use std::cmp::Reverse;
use std::collections::HashMap;

use thiserror::Error;

/// A single column of an archetype: the component type stored, referenced both by its
/// [`TypeKey`] (for layout metadata) and its [`ComponentKey`] (for storage metadata).
///
/// The layout and component fields are **frozen snapshots** taken at [`ArchetypeMeta::new`]
/// time: once the archetype is built, downstream layout computation reads size/align/drop info
/// straight from these fields and never re-queries the (dynamically modifiable) registries.
pub struct ColumnEntry {
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

impl ColumnEntry {
    /// Builds a column entry by resolving `type_key`/`comp_key` against the registries.
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

/// Metadata describing an archetype's column layout.
#[derive(Default)]
pub struct ArchetypeMeta {
    columns: Vec<ColumnEntry>,
    signature: ArchetypeSignature,
    id_to_index: HashMap<TypeId, usize>,
}

impl ArchetypeMeta {
    /// Builds [`ArchetypeMeta`] from `unsorted_columns`, which must already be validated (see
    /// [`ColumnEntry::new`]).
    ///
    /// The columns are reordered by **alignment descending**, then **size descending**, then
    /// **component key index ascending**. Because this is a total order over the layout
    /// attributes, the resulting column order — and the archetype layout it later drives — is
    /// independent of the order in which `unsorted_columns` was passed in.
    ///
    /// The [`ArchetypeSignature`] is computed here and frozen alongside the columns.
    pub fn new(unsorted_columns: Vec<ColumnEntry>) -> Self {
        let mut keyed: Vec<((Reverse<usize>, Reverse<usize>, usize), TypeId, ColumnEntry)> =
            Vec::with_capacity(unsorted_columns.len());
        for entry in unsorted_columns {
            keyed.push((
                (Reverse(entry.align), Reverse(entry.stride), entry.comp_key.index.as_usize()),
                entry.type_id,
                entry,
            ));
        }

        // alignment descending > size descending > component key index ascending
        keyed.sort_by(|a, b| a.0.cmp(&b.0));

        let mut columns = Vec::with_capacity(keyed.len());
        let mut id_to_index = HashMap::with_capacity(keyed.len());
        for (index, (_, id, entry)) in keyed.into_iter().enumerate() {
            id_to_index.insert(id, index);
            columns.push(entry);
        }

        let signature = {
            let mut signature = ArchetypeSignature::default();
            for column in &columns {
                signature.set(column.comp_key.index.as_usize());
            }
            signature
        };

        Self { columns, signature, id_to_index }
    }

    /// Columns in canonical order (see [`ArchetypeMeta::new`]).
    pub fn columns(&self) -> &[ColumnEntry] {
        &self.columns
    }

    /// Returns the canonical index of the column for `id`, if present.
    pub fn column_index(&self, id: &TypeId) -> Option<usize> {
        self.id_to_index.get(id).copied()
    }

    /// Resolves `id` to its column entry directly, if present.
    pub fn column(&self, id: &TypeId) -> Option<&ColumnEntry> {
        self.column_index(id).map(|index| &self.columns[index])
    }

    /// The [`ArchetypeSignature`] identifying this archetype's component set, computed at
    /// construction time.
    pub fn signature(&self) -> &ArchetypeSignature {
        &self.signature
    }
}

/// Errors returned when building an [`ArchetypeMeta`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A column's [`TypeKey`] does not resolve in the [`TypeRegistry`].
    #[error(transparent)]
    Type(#[from] crate::types::Error),

    /// A column's [`ComponentKey`] does not resolve in the [`ComponentRegistry`].
    #[error(transparent)]
    Component(#[from] crate::component::Error),

    /// A column's type key and component key resolve to different underlying types.
    #[error("column type key resolves to {type_id:?} but component key resolves to {comp_id:?}")]
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
        /// Builds a context containing the given (id, size, align) set. Component registration
        /// order matches the slice order, so comp key indices are 0..n.
        pub fn mock() -> Self {
            let mut type_reg = TypeRegistry::default();
            let mut comp_reg = ComponentRegistry::default();

            let types = [
                (TypeId::of_script(1), 8, 8),
                (TypeId::of_script(2), 4, 4),
                (TypeId::of_script(3), 2, 2),
                (TypeId::of_script(4), 16, 8),
                (TypeId::of_script(5), 8, 8),
                (TypeId::of_script(6), 4, 4),
                (TypeId::of_script(7), 4, 4),
            ];
            for (id, size, align) in types {
                type_reg.register(TypeMeta::new(id, size, align, TypeName::of_script("test")));
                comp_reg.register(ComponentMeta::new(id, ComponentKind::Trivial));
            }

            Self { type_reg, comp_reg }
        }

        pub fn column(&self, id: TypeId) -> ColumnEntry {
            let type_key = *self.type_reg.id_to_key(&id).unwrap();
            let comp_key = *self.comp_reg.id_to_key(&id).unwrap();
            ColumnEntry::new(type_key, comp_key, &self.type_reg, &self.comp_reg).unwrap()
        }

        /// Resolves the `TypeId` backing the column at `index`.
        pub fn column_id(&self, meta: &ArchetypeMeta, index: usize) -> TypeId {
            meta.columns()[index].type_id
        }
    }

    #[test]
    fn sorts_by_alignment_descending() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(vec![
            ctx.column(TypeId::of_script(3)), // align 2
            ctx.column(TypeId::of_script(1)), // align 8
            ctx.column(TypeId::of_script(2)), // align 4
        ]);

        assert_eq!(meta.columns().len(), 3);
        assert_eq!(ctx.column_id(&meta, 0), TypeId::of_script(1));
        assert_eq!(ctx.column_id(&meta, 1), TypeId::of_script(2));
        assert_eq!(ctx.column_id(&meta, 2), TypeId::of_script(3));
        assert_eq!(meta.column_index(&TypeId::of_script(1)).unwrap(), 0);
        assert_eq!(meta.column_index(&TypeId::of_script(2)).unwrap(), 1);
        assert_eq!(meta.column_index(&TypeId::of_script(3)).unwrap(), 2);
    }

    #[test]
    fn sorts_by_size_descending_within_same_alignment() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(vec![
            ctx.column(TypeId::of_script(5)), // align 8, size 8
            ctx.column(TypeId::of_script(4)), // align 8, size 16
        ]);

        assert_eq!(ctx.column_id(&meta, 0), TypeId::of_script(4));
        assert_eq!(ctx.column_id(&meta, 1), TypeId::of_script(5));
    }

    #[test]
    fn sorts_by_component_index_ascending_on_layout_tie() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(vec![
            ctx.column(TypeId::of_script(7)), // comp index 6
            ctx.column(TypeId::of_script(6)), // comp index 5
        ]);

        assert_eq!(ctx.column_id(&meta, 0), TypeId::of_script(6));
        assert_eq!(ctx.column_id(&meta, 1), TypeId::of_script(7));
    }

    #[test]
    fn ordering_is_independent_of_input_order() {
        let ctx = TestContext::mock();
        let ids = [
            TypeId::of_script(3),
            TypeId::of_script(1),
            TypeId::of_script(2),
            TypeId::of_script(5),
            TypeId::of_script(4),
            TypeId::of_script(7),
            TypeId::of_script(6),
        ];

        let forward = ArchetypeMeta::new(ids.iter().map(|id| ctx.column(*id)).collect());
        let reversed = ArchetypeMeta::new(ids.iter().rev().map(|id| ctx.column(*id)).collect());

        // align 8: [script 4 (size 16), script 1 (size 8, idx 0), script 5 (size 8, idx 4)]
        // align 4: [script 2 (idx 1), script 6 (idx 5), script 7 (idx 6)]
        // align 2: [script 3]
        let expected = [
            TypeId::of_script(4),
            TypeId::of_script(1),
            TypeId::of_script(5),
            TypeId::of_script(2),
            TypeId::of_script(6),
            TypeId::of_script(7),
            TypeId::of_script(3),
        ];

        for meta in [&forward, &reversed] {
            let order: Vec<TypeId> =
                (0..meta.columns().len()).map(|i| ctx.column_id(meta, i)).collect();
            assert_eq!(order, expected);
            for (index, id) in expected.iter().enumerate() {
                assert_eq!(meta.column_index(id).unwrap(), index);
            }
        }

        // The signature is a pure function of the component set, so both orders agree.
        assert_eq!(forward.signature(), reversed.signature());
    }

    #[test]
    fn rejects_mismatched_type_and_component_keys() {
        let ctx = TestContext::mock();
        let type_key = *ctx.type_reg.id_to_key(&TypeId::of_script(1)).unwrap();
        let comp_key = *ctx.comp_reg.id_to_key(&TypeId::of_script(2)).unwrap();

        let result = ColumnEntry::new(type_key, comp_key, &ctx.type_reg, &ctx.comp_reg);

        assert!(matches!(
            result,
            Err(Error::KeyMismatch { type_id, comp_id })
                if type_id == TypeId::of_script(1) && comp_id == TypeId::of_script(2)
        ));
    }

    #[test]
    fn rejects_unresolved_type_key() {
        let mut ctx = TestContext::mock();
        let column = ctx.column(TypeId::of_script(1));
        // Stale the type key so it no longer resolves.
        ctx.type_reg.unregister(column.type_key).unwrap();

        let result =
            ColumnEntry::new(column.type_key, column.comp_key, &ctx.type_reg, &ctx.comp_reg);

        assert!(matches!(result, Err(Error::Type(_))));
    }

    #[test]
    fn rejects_unresolved_component_key() {
        let mut ctx = TestContext::mock();
        let column = ctx.column(TypeId::of_script(1));
        // Stale the component key so it no longer resolves.
        ctx.comp_reg.unregister(column.comp_key).unwrap();

        let result =
            ColumnEntry::new(column.type_key, column.comp_key, &ctx.type_reg, &ctx.comp_reg);

        assert!(matches!(result, Err(Error::Component(_))));
    }

    #[test]
    fn signature_is_built_at_construction() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(vec![
            ctx.column(TypeId::of_script(1)), // comp index 0
            ctx.column(TypeId::of_script(3)), // comp index 2
            ctx.column(TypeId::of_script(5)), // comp index 4
        ]);

        let signature = meta.signature();

        assert_eq!(signature.count(), 3);
        for id in [
            TypeId::of_script(1),
            TypeId::of_script(3),
            TypeId::of_script(5),
        ] {
            let index = ctx.comp_reg.id_to_key(&id).unwrap().index.as_usize();
            assert!(signature.test(index), "bit {index} should be set");
        }
        for id in [
            TypeId::of_script(2),
            TypeId::of_script(4),
            TypeId::of_script(6),
            TypeId::of_script(7),
        ] {
            let index = ctx.comp_reg.id_to_key(&id).unwrap().index.as_usize();
            assert!(!signature.test(index), "bit {index} should be clear");
        }
    }

    #[test]
    fn signature_of_empty_archetype_is_empty() {
        let meta = ArchetypeMeta::new(Vec::new());

        assert!(meta.signature().is_empty());
        assert_eq!(meta.signature().count(), 0);
    }
}
