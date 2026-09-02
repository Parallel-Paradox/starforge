// Linker in MSVC prints an informational "Creating library ... and object ..."
// line to stdout when building this proc-macro's dll; silence the resulting lint.
#![allow(linker_messages)]

use proc_macro::TokenStream;
use syn::DeriveInput;

mod component;

#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();
    match component::impl_component(&ast) {
        Ok(stream) => stream,
        Err(err) => err.into_compile_error().into(),
    }
}
