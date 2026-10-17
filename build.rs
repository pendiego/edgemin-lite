// Embeds app.manifest (visual styles + DPI awareness) and app.rc (icon + version info). MSVC only.
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}\\app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/edgemin-lite.ico");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().unwrap();
}
