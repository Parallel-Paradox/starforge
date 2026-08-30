// Linker in MSVC prints an informational "Creating library ... and object ..."
// line to stdout when building this proc-macro's dll; silence the resulting lint.
#![allow(linker_messages)]

use proc_macro::TokenStream;
use quote::quote;
use starforge_macro_util::manifest::Manifest;
use syn::DeriveInput;

#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();
    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let starforge_core = Manifest::default().get_path("starforge_core");

    let stream = quote! {
        impl #impl_generics #starforge_core::component::Component for #ident #ty_generics
        #where_clause {}
    };
    stream.into()
}
