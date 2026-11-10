fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let linker_script = format!("{manifest_dir}/linker.ld");
    println!("cargo:rustc-link-arg=-T{linker_script}");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/boot.rs");
}
