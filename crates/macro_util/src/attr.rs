use syn::{DataStruct, Index, Member, Type};
use thiserror::Error;

pub fn find_unique_target_attr<'a>(
    struct_data: &'a DataStruct,
    target: &'static str,
) -> Result<(&'a Type, Member), UniqueAttrError> {
    let mut index = 0;
    let mut field_iter = struct_data.fields.iter();
    let mut result: Option<(&Type, Member)> = None;

    while let Some(field) = field_iter.next() {
        let attrs = &field.attrs;
        for attr in attrs {
            if !attr.path().is_ident(target) {
                continue;
            }
            if result.is_some() {
                return Err(UniqueAttrError::TooMuchMarkedField { target });
            }

            let member = field
                .ident
                .as_ref()
                .map(|name| Member::Named(name.clone()))
                .unwrap_or_else(|| Member::Unnamed(Index::from(index)));
            result = Some((&field.ty, member));
        }
        index += 1;
    }

    match result {
        Some(field) => Ok(field),
        None => Err(UniqueAttrError::NoMarkedField { target }),
    }
}

#[derive(Debug, Error)]
pub enum UniqueAttrError {
    #[error("Can't find field that marked #[{target}] attribute!")]
    NoMarkedField { target: &'static str },

    #[error("Only one field can be marked as #[{target}]!")]
    TooMuchMarkedField { target: &'static str },
}
