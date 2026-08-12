use std::cmp::Reverse;
use std::collections::HashMap;

use thiserror::Error;

use crate::{
    component::{ComponentKey, ComponentRegistry, ComponentRegistryError},
    types::{TypeId, TypeKey, TypeRegistry, TypeRegistryError},
};

/// A single column of an archetype: the component type stored, referenced both by its
/// `TypeKey` (for layout metadata) and its `ComponentKey` (for storage metadata).
pub struct ColumnEntry {
    pub type_key: TypeKey,
    pub comp_key: ComponentKey,
}

/// Metadata describing an archetype's column layout.
pub struct ArchetypeMeta {
    /// Columns in canonical order (see [`ArchetypeMeta::new`]).
    pub columns: Vec<ColumnEntry>,
    /// Maps each column's `TypeId` to its index in `columns`.
    pub id_to_index: HashMap<TypeId, usize>,
}

impl ArchetypeMeta {
    /// Builds `ArchetypeMeta` from `columns`.
    ///
    /// Each column's alignment and size are read from its registered `TypeMeta` via
    /// `type_registry`. The columns are then reordered by **alignment descending**, then
    /// **size descending**, then **component key index ascending**. Because this is a total
    /// order over the layout attributes, the resulting column order — and the archetype layout
    /// it later drives — is independent of the order in which `columns` was passed in.
    ///
    /// Every column's `type_key` and `comp_key` are resolved against the (dynamically
    /// modifiable) registries and checked to refer to the same underlying `TypeId`.
    ///
    /// # Errors
    ///
    /// Returns [`ArchetypeMetaError::Type`] or [`ArchetypeMetaError::Component`] if a column's
    /// key no longer resolves in its registry (e.g. the type was unregistered after the key was
    /// captured), and [`ArchetypeMetaError::KeyMismatch`] if a column's two keys refer to
    /// different types.
    pub fn new(
        columns: Vec<ColumnEntry>,
        type_registry: &TypeRegistry,
        comp_registry: &ComponentRegistry,
    ) -> Result<Self, ArchetypeMetaError> {
        // Resolve each column to its layout key and verify its type/component keys agree.
        let mut keyed: Vec<((Reverse<usize>, Reverse<usize>, u32), TypeId, ColumnEntry)> =
            Vec::with_capacity(columns.len());
        for entry in columns {
            let type_meta = type_registry.key_to_meta(&entry.type_key)?;
            let comp_meta = comp_registry.key_to_meta(&entry.comp_key)?;
            if type_meta.id() != comp_meta.id() {
                return Err(ArchetypeMetaError::KeyMismatch {
                    type_id: *type_meta.id(),
                    comp_id: *comp_meta.id(),
                });
            }
            keyed.push((
                (Reverse(type_meta.align()), Reverse(type_meta.size()), entry.comp_key.index),
                *type_meta.id(),
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

        Ok(Self { columns, id_to_index })
    }
}

/// Errors returned when building an [`ArchetypeMeta`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArchetypeMetaError {
    /// A column's `TypeKey` does not resolve in the `TypeRegistry`.
    #[error("column type key does not resolve: {0}")]
    Type(#[from] TypeRegistryError),

    /// A column's `ComponentKey` does not resolve in the `ComponentRegistry`.
    #[error("column component key does not resolve: {0}")]
    Component(#[from] ComponentRegistryError),

    /// A column's type key and component key resolve to different underlying types.
    #[error("column type key resolves to {type_id:?} but component key resolves to {comp_id:?}")]
    KeyMismatch { type_id: TypeId, comp_id: TypeId },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{ComponentKind, ComponentMeta};
    use crate::types::TypeMeta;

    /// Builds a type and component registry pair containing the given (id, size, align) set.
    /// Component registration order matches the slice order, so comp key indices are 0..n.
    fn registries() -> (TypeRegistry, ComponentRegistry) {
        let mut type_reg = TypeRegistry::default();
        let mut comp_reg = ComponentRegistry::default();

        // script 1: align 8, size 8
        // script 2: align 4, size 4
        // script 3: align 2, size 2
        // script 4: align 8, size 16
        // script 5: align 8, size 8
        // script 6: align 4, size 4
        // script 7: align 4, size 4
        let types = [
            (TypeId::of_script(1), 8usize, 8usize),
            (TypeId::of_script(2), 4, 4),
            (TypeId::of_script(3), 2, 2),
            (TypeId::of_script(4), 16, 8),
            (TypeId::of_script(5), 8, 8),
            (TypeId::of_script(6), 4, 4),
            (TypeId::of_script(7), 4, 4),
        ];
        for (id, size, align) in types {
            type_reg.register(TypeMeta::new(id, size, align, "test"));
            comp_reg.register(ComponentMeta::new(id, ComponentKind::Trivial));
        }

        (type_reg, comp_reg)
    }

    fn column(type_reg: &TypeRegistry, comp_reg: &ComponentRegistry, id: TypeId) -> ColumnEntry {
        let type_key = *type_reg.id_to_key(&id).unwrap();
        let comp_key = *comp_reg.id_to_key(&id).unwrap();
        ColumnEntry { type_key, comp_key }
    }

    /// Resolves the `TypeId` backing the column at `index`.
    fn column_id(meta: &ArchetypeMeta, type_reg: &TypeRegistry, index: usize) -> TypeId {
        *type_reg
            .key_to_meta(&meta.columns[index].type_key)
            .unwrap()
            .id()
    }

    #[test]
    fn sorts_by_alignment_descending() {
        let (type_reg, comp_reg) = registries();
        let meta = ArchetypeMeta::new(
            vec![
                column(&type_reg, &comp_reg, TypeId::of_script(3)), // align 2
                column(&type_reg, &comp_reg, TypeId::of_script(1)), // align 8
                column(&type_reg, &comp_reg, TypeId::of_script(2)), // align 4
            ],
            &type_reg,
            &comp_reg,
        )
        .unwrap();

        assert_eq!(meta.columns.len(), 3);
        assert_eq!(column_id(&meta, &type_reg, 0), TypeId::of_script(1));
        assert_eq!(column_id(&meta, &type_reg, 1), TypeId::of_script(2));
        assert_eq!(column_id(&meta, &type_reg, 2), TypeId::of_script(3));
        assert_eq!(meta.id_to_index[&TypeId::of_script(1)], 0);
        assert_eq!(meta.id_to_index[&TypeId::of_script(2)], 1);
        assert_eq!(meta.id_to_index[&TypeId::of_script(3)], 2);
    }

    #[test]
    fn sorts_by_size_descending_within_same_alignment() {
        let (type_reg, comp_reg) = registries();
        let meta = ArchetypeMeta::new(
            vec![
                column(&type_reg, &comp_reg, TypeId::of_script(5)), // align 8, size 8
                column(&type_reg, &comp_reg, TypeId::of_script(4)), // align 8, size 16
            ],
            &type_reg,
            &comp_reg,
        )
        .unwrap();

        assert_eq!(column_id(&meta, &type_reg, 0), TypeId::of_script(4));
        assert_eq!(column_id(&meta, &type_reg, 1), TypeId::of_script(5));
    }

    #[test]
    fn sorts_by_component_index_ascending_on_layout_tie() {
        let (type_reg, comp_reg) = registries();
        let meta = ArchetypeMeta::new(
            vec![
                column(&type_reg, &comp_reg, TypeId::of_script(7)), // comp index 6
                column(&type_reg, &comp_reg, TypeId::of_script(6)), // comp index 5
            ],
            &type_reg,
            &comp_reg,
        )
        .unwrap();

        assert_eq!(column_id(&meta, &type_reg, 0), TypeId::of_script(6));
        assert_eq!(column_id(&meta, &type_reg, 1), TypeId::of_script(7));
    }

    #[test]
    fn ordering_is_independent_of_input_order() {
        let (type_reg, comp_reg) = registries();
        let ids = [
            TypeId::of_script(3),
            TypeId::of_script(1),
            TypeId::of_script(2),
            TypeId::of_script(5),
            TypeId::of_script(4),
            TypeId::of_script(7),
            TypeId::of_script(6),
        ];

        let forward = ArchetypeMeta::new(
            ids.iter()
                .map(|id| column(&type_reg, &comp_reg, *id))
                .collect(),
            &type_reg,
            &comp_reg,
        )
        .unwrap();
        let reversed = ArchetypeMeta::new(
            ids.iter()
                .rev()
                .map(|id| column(&type_reg, &comp_reg, *id))
                .collect(),
            &type_reg,
            &comp_reg,
        )
        .unwrap();

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
            let order: Vec<TypeId> = (0..meta.columns.len())
                .map(|i| column_id(meta, &type_reg, i))
                .collect();
            assert_eq!(order, expected);
            for (index, id) in expected.iter().enumerate() {
                assert_eq!(meta.id_to_index[id], index);
            }
        }
    }

    #[test]
    fn rejects_mismatched_type_and_component_keys() {
        let (type_reg, comp_reg) = registries();
        let type_key = *type_reg.id_to_key(&TypeId::of_script(1)).unwrap();
        let comp_key = *comp_reg.id_to_key(&TypeId::of_script(2)).unwrap();

        let result =
            ArchetypeMeta::new(vec![ColumnEntry { type_key, comp_key }], &type_reg, &comp_reg);

        assert!(matches!(
            result,
            Err(ArchetypeMetaError::KeyMismatch { type_id, comp_id })
                if type_id == TypeId::of_script(1) && comp_id == TypeId::of_script(2)
        ));
    }

    #[test]
    fn rejects_unresolved_type_key() {
        let (mut type_reg, comp_reg) = registries();
        let type_key = *type_reg.id_to_key(&TypeId::of_script(1)).unwrap();
        let comp_key = *comp_reg.id_to_key(&TypeId::of_script(1)).unwrap();
        // Stale the type key so it no longer resolves.
        type_reg.unregister(type_key).unwrap();

        let result =
            ArchetypeMeta::new(vec![ColumnEntry { type_key, comp_key }], &type_reg, &comp_reg);

        assert!(matches!(result, Err(ArchetypeMetaError::Type(_))));
    }

    #[test]
    fn rejects_unresolved_component_key() {
        let (type_reg, mut comp_reg) = registries();
        let type_key = *type_reg.id_to_key(&TypeId::of_script(1)).unwrap();
        let comp_key = *comp_reg.id_to_key(&TypeId::of_script(1)).unwrap();
        // Stale the component key so it no longer resolves.
        comp_reg.unregister(comp_key).unwrap();

        let result =
            ArchetypeMeta::new(vec![ColumnEntry { type_key, comp_key }], &type_reg, &comp_reg);

        assert!(matches!(result, Err(ArchetypeMetaError::Component(_))));
    }
}
