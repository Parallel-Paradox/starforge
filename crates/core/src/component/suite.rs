use crate::prelude::Component;
use starforge_reflect::basic::Parcel;
use starforge_reflect::prelude::TypeId;
use std::collections::HashMap;

/// A collection of components that can be stored in an entity.
#[derive(Default)]
pub struct ComponentSuite(HashMap<TypeId, Parcel>);

impl ComponentSuite {
    pub fn insert<T: Component>(&mut self, component: T) -> Option<T> {
        self.insert_impl(TypeId::of::<T>(), Parcel::new(component))
            // SAFETY: popped by the same TypeId
            .map(|parcel| unsafe { parcel.take() })
    }

    pub fn insert_impl(&mut self, id: TypeId, parcel: Parcel) -> Option<Parcel> {
        self.0.insert(id, parcel)
    }

    pub fn remove<T: Component>(&mut self, id: &TypeId) -> Option<T> {
        // SAFETY: popped by the same TypeId
        self.remove_impl(id).map(|parcel| unsafe { parcel.take() })
    }

    pub fn remove_impl(&mut self, id: &TypeId) -> Option<Parcel> {
        self.0.remove(id)
    }
}
