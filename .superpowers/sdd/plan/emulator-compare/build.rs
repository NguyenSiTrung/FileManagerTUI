use std::env;
use std::fs;
use std::path::PathBuf;

// Pull the repo's emulator source into this crate verbatim except for the
// leading `//!` module docs, which `include!` cannot carry. The transform is
// comment-style only: `//!` lines become `//` lines.
fn main() {
    let source = PathBuf::from("../../../../src/terminal/emulator.rs");
    let text = fs::read_to_string(&source).expect("read emulator.rs");
    let rewritten: String = text
        .lines()
        .map(|line| {
            line.strip_prefix("//!")
                .map(|rest| format!("//{rest}"))
                .unwrap_or_else(|| line.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("emulator_inc.rs");
    fs::write(&out, rewritten).expect("write emulator_inc.rs");
    println!("cargo:rerun-if-changed={}", source.display());
}
