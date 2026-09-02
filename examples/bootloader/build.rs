use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    // `link.x` (provided by qingke-rt) does `INCLUDE memory.x`, so the directory
    // holding our `memory.x` has to be on the linker search path.
    let memory_dir = manifest.join("memory");
    // Our `memory.x` in turn does `INCLUDE ch32v305rbt6.x`. That file is shared
    // with the application example so that the two binaries can never disagree
    // about where the partitions live.
    let partition_dir = manifest.join("../../partition-map");

    println!("cargo:rustc-link-search=native={}", memory_dir.display());
    println!("cargo:rustc-link-search=native={}", partition_dir.display());

    println!("cargo:rustc-link-arg-bins=-Tlink.x");

    println!(
        "cargo:rerun-if-changed={}",
        memory_dir.join("memory.x").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        partition_dir.join("ch32v305rbt6.x").display()
    );
    println!("cargo:rerun-if-changed=build.rs");
}
