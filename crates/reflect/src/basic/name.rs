use std::{any::Any, sync::Arc};

/// A human-readable name for a native Rust type or a script-defined type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeName {
    /// The name reported by Rust for a native type.
    Native(&'static str),
    /// The name assigned to a script-defined type.
    Script(Arc<str>),
}

impl TypeName {
    /// Returns the Rust type name for a native type `T`.
    pub fn of<T: Any>() -> Self {
        Self::Native(std::any::type_name::<T>())
    }

    /// Returns the name of a script-defined type.
    pub fn of_script(name: impl AsRef<str>) -> Self {
        Self::Script(Arc::from(name.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_name_is_derived_from_rust_type_name() {
        assert_eq!(TypeName::of::<u8>(), TypeName::Native("u8"));
    }

    #[test]
    fn script_name_accepts_string_like_values() {
        let from_str = TypeName::of_script("Player");
        let from_string = TypeName::of_script(String::from("Player"));
        let from_arc = TypeName::Script(Arc::from("Player"));
        assert_eq!(from_str, from_string);
        assert_eq!(from_str, from_arc);
    }
}
