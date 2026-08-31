use super::ArchetypeSignature;
use crate::{
    component::{ComponentRegistry, Error},
    prelude::ComponentKey,
};

use std::cmp::Ordering;
use std::collections::HashMap;

use starforge_reflect::prelude::{TypeId, TypeMeta};

pub type ColumnEntry = (ComponentKey, TypeMeta);

/// Orders [`ColumnEntry`]s by **alignment descending**, then **size descending**, then
/// **component key index ascending**.
///
/// Because the order is a total function of the layout attributes and key indices, sorting
/// with this comparator produces a canonical column order independent of input order.
fn cmp_columns(a: &ColumnEntry, b: &ColumnEntry) -> Ordering {
    let (a_key, a_meta) = a;
    let (b_key, b_meta) = b;
    let a_layout = a_meta.layout();
    let b_layout = b_meta.layout();
    b_layout
        .align()
        .cmp(&a_layout.align())
        .then_with(|| b_layout.size().cmp(&a_layout.size()))
        .then_with(|| a_key.index.get().cmp(&b_key.index.get()))
}

/// Metadata describing an archetype's column layout.
#[derive(Default)]
pub struct ArchetypeMeta {
    columns: Vec<ColumnEntry>,
    signature: ArchetypeSignature,
    id_to_index: HashMap<TypeId, usize>,
}

impl ArchetypeMeta {
    /// Builds [`ArchetypeMeta`] by resolving each component key in `unsorted_columns` to its
    /// [`TypeMeta`] through `comp_reg`; unknown or stale keys are reported as [`Error`].
    ///
    /// The columns are sorted in place with [`cmp_columns`] — a total order over the layout
    /// attributes — so the resulting column order, and the archetype layout it later drives,
    /// is independent of the order in which `unsorted_columns` was passed in.
    ///
    /// The [`ArchetypeSignature`] is computed here and frozen alongside the columns.
    pub fn new(
        unsorted_columns: &[ComponentKey],
        comp_reg: &ComponentRegistry,
    ) -> Result<Self, Error> {
        let mut columns: Vec<ColumnEntry> = Vec::with_capacity(unsorted_columns.len());
        for key in unsorted_columns {
            let meta = comp_reg.key_to_meta(key)?;
            columns.push((*key, meta.clone()));
        }
        columns.sort_by(cmp_columns);

        let mut id_to_index = HashMap::with_capacity(columns.len());
        for (index, (_, meta)) in columns.iter().enumerate() {
            id_to_index.insert(meta.id(), index);
        }

        let signature = {
            let mut signature = ArchetypeSignature::default();
            for (key, _) in &columns {
                signature.set(key.index.get() as usize);
            }
            signature
        };

        Ok(Self { columns, signature, id_to_index })
    }

    /// Columns in canonical order (see [`ArchetypeMeta::new`]).
    pub fn columns(&self) -> &[ColumnEntry] {
        &self.columns
    }

    /// Byte size of one element of the `index`-th column.
    pub fn column_size(&self, index: usize) -> usize {
        self.columns[index].1.layout().size()
    }

    /// Byte alignment of the `index`-th column's element.
    pub fn column_align(&self, index: usize) -> usize {
        self.columns[index].1.layout().align()
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

#[cfg(test)]
mod tests {
    use super::*;
    use starforge_reflect::basic::meta::NeedsDrop;
    use starforge_reflect::prelude::TypeName;
    use std::alloc::Layout;

    struct TestContext {
        comp_reg: ComponentRegistry,
    }

    impl TestContext {
        /// Builds a context containing the given (id, size, align) set. Component registration
        /// order matches the slice order, so comp key indices are 0..n.
        pub fn mock() -> Self {
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
                comp_reg.register(TypeMeta::new_impl(
                    id,
                    TypeName::of_script("test"),
                    NeedsDrop::Trivial,
                    Layout::from_size_align(size, align).unwrap(),
                ));
            }

            Self { comp_reg }
        }

        /// Resolves `id` to its registered [`ComponentKey`].
        pub fn column(&self, id: TypeId) -> ComponentKey {
            *self.comp_reg.id_to_key(&id).unwrap()
        }

        /// Resolves the `TypeId` backing the column at `index`.
        pub fn column_id(&self, meta: &ArchetypeMeta, index: usize) -> TypeId {
            meta.columns()[index].1.id()
        }
    }

    #[test]
    fn sorts_by_alignment_descending() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(
            &[
                ctx.column(TypeId::of_script(3)), // align 2
                ctx.column(TypeId::of_script(1)), // align 8
                ctx.column(TypeId::of_script(2)), // align 4
            ],
            &ctx.comp_reg,
        )
        .unwrap();

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
        let meta = ArchetypeMeta::new(
            &[
                ctx.column(TypeId::of_script(5)), // align 8, size 8
                ctx.column(TypeId::of_script(4)), // align 8, size 16
            ],
            &ctx.comp_reg,
        )
        .unwrap();

        assert_eq!(ctx.column_id(&meta, 0), TypeId::of_script(4));
        assert_eq!(ctx.column_id(&meta, 1), TypeId::of_script(5));
    }

    #[test]
    fn sorts_by_component_index_ascending_on_layout_tie() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(
            &[
                ctx.column(TypeId::of_script(7)), // comp index 6
                ctx.column(TypeId::of_script(6)), // comp index 5
            ],
            &ctx.comp_reg,
        )
        .unwrap();

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

        let forward_keys: Vec<ComponentKey> = ids.iter().map(|id| ctx.column(*id)).collect();
        let reversed_keys: Vec<ComponentKey> = ids.iter().rev().map(|id| ctx.column(*id)).collect();
        let forward = ArchetypeMeta::new(&forward_keys, &ctx.comp_reg).unwrap();
        let reversed = ArchetypeMeta::new(&reversed_keys, &ctx.comp_reg).unwrap();

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
    fn rejects_unresolved_component_key() {
        let mut ctx = TestContext::mock();
        let column = ctx.column(TypeId::of_script(1));
        // Stale the component key so it no longer resolves.
        ctx.comp_reg.unregister(column).unwrap();

        let result = ArchetypeMeta::new(&[column], &ctx.comp_reg);

        assert!(matches!(result, Err(Error::GenerationMismatch { .. })));
    }

    #[test]
    fn signature_is_built_at_construction() {
        let ctx = TestContext::mock();
        let meta = ArchetypeMeta::new(
            &[
                ctx.column(TypeId::of_script(1)), // comp index 0
                ctx.column(TypeId::of_script(3)), // comp index 2
                ctx.column(TypeId::of_script(5)), // comp index 4
            ],
            &ctx.comp_reg,
        )
        .unwrap();

        let signature = meta.signature();

        assert_eq!(signature.count(), 3);
        for id in [
            TypeId::of_script(1),
            TypeId::of_script(3),
            TypeId::of_script(5),
        ] {
            let index = ctx.comp_reg.id_to_key(&id).unwrap().index.get() as usize;
            assert!(signature.test(index), "bit {index} should be set");
        }
        for id in [
            TypeId::of_script(2),
            TypeId::of_script(4),
            TypeId::of_script(6),
            TypeId::of_script(7),
        ] {
            let index = ctx.comp_reg.id_to_key(&id).unwrap().index.get() as usize;
            assert!(!signature.test(index), "bit {index} should be clear");
        }
    }

    #[test]
    fn signature_of_empty_archetype_is_empty() {
        let comp_reg = ComponentRegistry::default();
        let meta = ArchetypeMeta::new(&[], &comp_reg).unwrap();

        assert!(meta.signature().is_empty());
        assert_eq!(meta.signature().count(), 0);
    }
}
