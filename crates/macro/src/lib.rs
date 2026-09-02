// Linker in MSVC prints an informational "Creating library ... and object ..."
// line to stdout when building this proc-macro's dll; silence the resulting lint.
#![allow(linker_messages)]

use proc_macro::TokenStream;

mod deref;

const DEREF_TARGET_ATTR: &str = "deref";

#[proc_macro_derive(Deref, attributes(deref))]
pub fn derive_deref(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    deref::impl_deref_macro(&ast)
}

#[proc_macro_derive(DerefMut)]
pub fn derive_deref_mut(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    deref::impl_deref_mut_macro(&ast)
}
