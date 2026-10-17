// Embeds app.manifest (visual styles + DPI awareness). MSVC linker only.
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}\\app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
}
