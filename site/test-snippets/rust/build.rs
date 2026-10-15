//! Discovers every `*.rs` file under `site/src/content/docs` and mounts each one as a module of
//! this crate, so `cargo check` type-checks all Rust doc snippets at once.

use std::{env, fs, path::Path};

fn main() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/content/docs");
    let docs = docs.canonicalize().expect("site/src/content/docs exists");
    let pattern = format!("{}/**/*.rs", docs.display());

    let mut out = String::new();
    for entry in glob::glob(&pattern).expect("valid glob") {
        let path = entry.expect("readable path");
        println!("cargo:rerun-if-changed={}", path.display());
        let rel = path.strip_prefix(&docs).unwrap().with_extension("");
        let name = rel
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>();
        out.push_str(&format!("#[path = {:?}]\nmod snippet_{name};\n", path.display()));
    }
    // A new snippet file only changes its parent directory's mtime, so watch every directory.
    for entry in glob::glob(&format!("{}/**/", docs.display())).expect("valid glob") {
        println!("cargo:rerun-if-changed={}", entry.expect("readable path").display());
    }

    let dest = Path::new(&env::var("OUT_DIR").unwrap()).join("snippets.rs");
    fs::write(dest, out).expect("write snippets.rs");
}
