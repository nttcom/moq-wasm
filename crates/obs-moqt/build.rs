use std::env;

fn main() {
    // libobs symbols are resolved when OBS loads the plugin. The macOS linker
    // rejects undefined symbols in a dynamic library unless told otherwise.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }

    // The dock needs Qt 6 headers, so it is built only when
    // `OBS_MOQT_QT_CFLAGS` holds the compiler flags that find them. Qt's
    // symbols, like libobs's, are resolved when OBS loads the plugin.
    println!("cargo:rustc-check-cfg=cfg(dock)");
    println!("cargo:rerun-if-env-changed=OBS_MOQT_QT_CFLAGS");
    println!("cargo:rerun-if-changed=src/dock.cpp");
    let qt_cflags = env::var("OBS_MOQT_QT_CFLAGS").unwrap_or_default();
    if qt_cflags.trim().is_empty() {
        return;
    }
    let mut dock = cc::Build::new();
    dock.cpp(true).std("c++17").file("src/dock.cpp");
    for flag in qt_cflags.split_whitespace() {
        dock.flag(flag);
    }
    dock.compile("obs_moqt_dock");
    println!("cargo:rustc-cfg=dock");
}
