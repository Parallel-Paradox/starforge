use proc_macro::{Span, TokenStream};
use quote::quote;
use starforge_macro_util::attr::find_unique_target_attr;
use syn::{self, Data, DeriveInput, Index, Member, Type};

use crate::DEREF_TARGET_ATTR;

pub fn impl_deref_macro(ast: &DeriveInput) -> TokenStream {
    let ident = &ast.ident;

    let (deref_type, member) = match get_target_field(ast) {
        Ok(field) => field,
        Err(err) => {
            return err.into_compile_error().into();
        }
    };

    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let stream = quote! {
        impl #impl_generics std::ops::Deref for #ident #ty_generics #where_clause {
            type Target = #deref_type;

            fn deref(&self) -> &Self::Target {
                return &self.#member;
            }
        }
    };

    stream.into()
}

pub fn impl_deref_mut_macro(ast: &DeriveInput) -> TokenStream {
    let ident = &ast.ident;

    let (_, member) = match get_target_field(ast) {
        Ok(field) => field,
        Err(err) => {
            return err.into_compile_error().into();
        }
    };

    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let stream = quote! {
        impl #impl_generics std::ops::DerefMut for #ident #ty_generics #where_clause {
            fn deref_mut(&mut self) -> &mut Self::Target {
                return &mut self.#member;
            }
        }
    };

    stream.into()
}

fn get_target_field(ast: &DeriveInput) -> syn::Result<(&Type, Member)> {
    let s = match &ast.data {
        Data::Struct(s) => s,
        _ => {
            let message = "Deref macro can only be derived on struct!";
            return Err(syn::Error::new(Span::call_site().into(), message));
        }
    };

    match s.fields.len() {
        0 => {
            let message = "Can't deref a empty struct!";
            Err(syn::Error::new(Span::call_site().into(), message))
        }
        1 => {
            let field = s.fields.iter().next().unwrap();
            let member = field
                .ident
                .as_ref()
                .map(|name| Member::Named(name.clone()))
                .unwrap_or_else(|| Member::Unnamed(Index::from(0)));
            Ok((&field.ty, member))
        }
        _ => {
            let err = match find_unique_target_attr(s, DEREF_TARGET_ATTR) {
                Ok(result) => {
                    return Ok(result);
                }
                Err(err) => err,
            };
            Err(syn::Error::new(Span::call_site().into(), err))
        }
    }
}
