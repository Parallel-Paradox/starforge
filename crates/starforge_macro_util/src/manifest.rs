use std::{env, path::PathBuf, str::FromStr};

use proc_macro::TokenStream;
use toml::Value;

extern crate proc_macro;

/// Parsed `Cargo.toml` of the crate currently being compiled, used to resolve dependency paths
/// in proc-macros.
pub struct Manifest(Value);

impl Default for Manifest {
    /// Reads and parses `Cargo.toml` from `CARGO_MANIFEST_DIR`.
    ///
    /// # Panics
    /// Panics if `CARGO_MANIFEST_DIR` is unset, or the file can't be read or parsed as TOML.
    fn default() -> Self {
        let value = env::var_os("CARGO_MANIFEST_DIR")
            .map(|manifest_dir| {
                let mut path = PathBuf::from(manifest_dir);
                path.push("Cargo.toml");
                let content = std::fs::read_to_string(path).unwrap();
                toml::from_str::<Value>(&content).unwrap()
            })
            .unwrap();
        Manifest(value)
    }
}

impl Manifest {
    /// Same as [`Self::try_get_path`], panicking if `name` can't be resolved.
    pub fn get_path(&self, name: &'static str) -> syn::Path {
        self.try_get_path(name).unwrap()
    }

    /// Resolves `name` to a usable path: `crate` if it's the current package, otherwise the
    /// dependency name itself if it's listed under `[dependencies]` or `[dev-dependencies]`.
    pub fn try_get_path(&self, name: &'static str) -> Option<syn::Path> {
        let package_name = self.0.get("package")?.get("name")?.as_str()?;
        if name == package_name {
            return parse_str_to_path("crate");
        }
        if let Some(deps) = self.0.get("dependencies") {
            if deps.get(name).is_some() {
                return parse_str_to_path(name);
            }
        }
        if let Some(deps) = self.0.get("dev-dependencies") {
            if deps.get(name).is_some() {
                return parse_str_to_path(name);
            }
        }
        None
    }
}

/// Parses `str` as a Rust path expression (e.g. `crate` or a crate name).
fn parse_str_to_path(str: &str) -> Option<syn::Path> {
    syn::parse::<syn::Path>(TokenStream::from_str(str).ok()?).ok()
}
