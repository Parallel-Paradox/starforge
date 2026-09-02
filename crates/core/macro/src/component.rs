use proc_macro::TokenStream;
use quote::quote;
use starforge_macro_util::attr::find_unique_attr;
use starforge_macro_util::manifest::Manifest;
use syn::meta::{ParseNestedMeta, parser};
use syn::{DeriveInput, Ident, Path};

/// Helper attribute, e.g. `#[component(storage = SparseSet)]`.
const COMPONENT_ATTR: &str = "component";

/// Generates the `Component` impl for `ast`, honoring `#[component(storage = ...)]`.
pub fn impl_component(ast: &DeriveInput) -> syn::Result<TokenStream> {
    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let starforge_core = Manifest::default().get_path("starforge_core");

    let storage = match parse_storage_attr(ast)? {
        Some(storage) => quote! {
            fn storage() -> #starforge_core::component::ComponentStorage {
                #starforge_core::component::ComponentStorage::#storage
            }
        },
        // No `#[component(storage = ...)]`; rely on the trait's `Archetype` default.
        None => quote! {},
    };

    let stream = quote! {
        impl #impl_generics #starforge_core::component::Component for #ident #ty_generics
        #where_clause {
            #storage
        }
    };
    Ok(stream.into())
}

/// Extracts the storage variant from `#[component(storage = ...)]`, if present.
fn parse_storage_attr(ast: &DeriveInput) -> syn::Result<Option<Ident>> {
    let mut storage: Option<Ident> = None;
    if let Some(attr) = find_unique_attr(&ast.attrs, COMPONENT_ATTR)? {
        attr.parse_args_with(parser(|meta: ParseNestedMeta| {
            if !meta.path.is_ident("storage") {
                return Err(meta.error("unsupported `#[component]` key; expected `storage = ...`"));
            }
            if storage.is_some() {
                return Err(meta.error("duplicate `storage` key in `#[component]`"));
            }
            storage = Some(parse_storage_value(meta)?);
            Ok(())
        }))?;
    }
    Ok(storage)
}

/// Parses the value of a `storage = ...` key, validating it against
/// [`ComponentStorage`]'s variants.
fn parse_storage_value(meta: ParseNestedMeta) -> syn::Result<Ident> {
    let value_stream = meta.value()?;
    let value_path: Path = value_stream.parse()?;
    let variant = value_path.get_ident().ok_or_else(|| {
        meta.error("`storage` expects a storage variant like `Archetype` or `SparseSet`")
    })?;

    match variant.to_string().as_str() {
        "Archetype" | "SparseSet" => Ok(variant.clone()),
        _ => {
            Err(meta
                .error(format!("unknown storage `{variant}`; expected `Archetype` or `SparseSet`")))
        }
    }
}
