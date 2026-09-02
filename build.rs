//! Build script that rejects impossible feature combinations.
//!
//! `Cargo.toml` has no syntax for "these two features are exclusive", so the
//! check is done here: `cargo::error=` (Rust 1.84+, so comfortably below the
//! 1.85 that edition 2024 requires anyway) fails *any* build of this crate -
//! `check`, `clippy`, `doc`, `build` - with a message that says what to do.
//!
//! The build script also runs while `embassy-boot` is being compiled, so the
//! build reports this message next to - rather than only - the generic
//! `compile_error!` from `embassy-boot`'s `fmt` module.

fn main() {
    println!("cargo::rerun-if-changed=Cargo.toml");

    let log = std::env::var_os("CARGO_FEATURE_LOG").is_some();
    let defmt = std::env::var_os("CARGO_FEATURE_DEFMT").is_some();

    if log && defmt {
        println!(
            "cargo::error=the `log` and `defmt` features of `embassy-boot-ch32` are mutually exclusive; enable at most one of them"
        );
    }
}
