fn main() {
    // libobs symbols are resolved when OBS loads the plugin. The macOS linker
    // rejects undefined symbols in a dynamic library unless told otherwise.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }
}
