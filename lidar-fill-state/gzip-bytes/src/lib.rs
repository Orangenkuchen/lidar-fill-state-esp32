use flate2::write::GzEncoder;
use flate2::Compression;
use proc_macro::{Span, TokenStream};
use quote::quote;
use std::io::Write;
use std::path::Path;

#[proc_macro]
pub fn gzip_bytes(input: TokenStream) -> TokenStream {
    let path = syn::parse_macro_input!(input as syn::LitStr);
    let relative_path = path.value();

    // Get the Rust source file containing this macro invocation.
    let source_file = Span::call_site().file();
    let source_dir = Path::new(&source_file)
        .parent()
        .expect("Could not determine source directory");

    let path = source_dir.join(&relative_path);

    eprintln!("[gzip_bytes] Reading {}", path.display());

    let data = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&data).expect("Gzip compression failed");

    let compressed = encoder.finish().expect("Gzip finalization failed");

    eprintln!(
        "[gzip_bytes] {} bytes -> {} bytes",
        data.len(),
        compressed.len()
    );

    let bytes = compressed.iter();
    quote! { &[#(#bytes),*] as &[u8] }.into()
}
