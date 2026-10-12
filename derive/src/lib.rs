//! The proc-macro behind `#[derive(FunctionConfig)]` (design R44).

use proc_macro::TokenStream;

/// Derives `FunctionConfig` for a function's typed configuration. Emits
/// nothing yet.
#[proc_macro_derive(FunctionConfig, attributes(config, output))]
pub fn derive_function_config(input: TokenStream) -> TokenStream {
    let _ = syn::parse_macro_input!(input as syn::DeriveInput);
    TokenStream::new()
}
