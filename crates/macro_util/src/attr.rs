use syn::{Attribute, DataStruct, Index, Member, Type};
use thiserror::Error;

/// Finds the single attribute in `attrs` whose path is `target`, reporting
/// duplicate occurrences.
///
/// Returns `Ok(None)` when no attribute matches, `Ok(Some(attr))` when exactly
/// one matches, and [`UniqueAttrError::TooManyAttrs`] when several do.
pub fn find_unique_attr<'a>(
    attrs: &'a [Attribute],
    target: &'static str,
) -> Result<Option<&'a Attribute>, UniqueAttrError> {
    let mut result: Option<&'a Attribute> = None;
    for attr in attrs {
        if !attr.path().is_ident(target) {
            continue;
        }
        if result.is_some() {
            return Err(UniqueAttrError::TooManyAttrs { target });
        }
        result = Some(attr);
    }
    Ok(result)
}

/// Finds the unique struct field marked with `#[{target}]`, reporting missing or
/// duplicate occurrences.
pub fn find_unique_target_attr<'a>(
    struct_data: &'a DataStruct,
    target: &'static str,
) -> Result<(&'a Type, Member), UniqueAttrError> {
    let mut result: Option<(&Type, Member)> = None;

    for (index, field) in struct_data.fields.iter().enumerate() {
        if find_unique_attr(&field.attrs, target)?.is_some() {
            if result.is_some() {
                return Err(UniqueAttrError::TooManyAttrs { target });
            }

            let member = field
                .ident
                .as_ref()
                .map(|name| Member::Named(name.clone()))
                .unwrap_or_else(|| Member::Unnamed(Index::from(index)));
            result = Some((&field.ty, member));
        }
    }

    match result {
        Some(field) => Ok(field),
        None => Err(UniqueAttrError::NoSuchAttr { target }),
    }
}

#[derive(Debug, Error)]
pub enum UniqueAttrError {
    #[error("Can't find an attribute marked #[{target}]!")]
    NoSuchAttr { target: &'static str },

    #[error("Only one attribute can be marked as #[{target}]!")]
    TooManyAttrs { target: &'static str },
}

impl From<UniqueAttrError> for syn::Error {
    fn from(err: UniqueAttrError) -> Self {
        syn::Error::new(proc_macro2::Span::call_site(), err)
    }
}
