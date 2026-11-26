use std::{env, fs, path::Path};

/// The libobs and obs-frontend-api functions `dock.cpp` declares, by DLL.
const DOCK_IMPORTS: [(&str, &[&str]); 2] = [
    (
        "obs",
        &[
            "obs_data_create",
            "obs_data_release",
            "obs_data_set_string",
            "obs_data_set_bool",
            "obs_data_get_string",
            "obs_data_get_bool",
            "obs_service_create",
            "obs_service_release",
            "obs_service_get_id",
            "obs_service_get_settings",
        ],
    ),
    (
        "obs-frontend-api",
        &[
            "obs_frontend_add_dock_by_id",
            "obs_frontend_add_event_callback",
            "obs_frontend_remove_event_callback",
            "obs_frontend_get_streaming_service",
            "obs_frontend_set_streaming_service",
            "obs_frontend_save_streaming_service",
            "obs_frontend_streaming_active",
        ],
    ),
];

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // libobs symbols are resolved when OBS loads the plugin. The macOS linker
    // rejects undefined symbols in a dynamic library unless told otherwise;
    // on Windows `libobs.rs` imports them by DLL name.
    if target_os == "macos" {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }

    // The dock needs Qt 6 headers, so it is built only when
    // `OBS_MOQT_QT_CFLAGS` holds the compiler flags that find them. Qt's
    // symbols, like libobs's, are resolved when OBS loads the plugin.
    println!("cargo:rustc-check-cfg=cfg(dock)");
    println!("cargo:rerun-if-env-changed=OBS_MOQT_QT_CFLAGS");
    println!("cargo:rerun-if-env-changed=OBS_MOQT_QT_LIB_DIR");
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
    if target_os == "windows" {
        // Qt's headers refuse MSVC without these two.
        dock.flag("/permissive-").flag("/Zc:__cplusplus");
        link_dock_imports();
    }
    dock.compile("obs_moqt_dock");
    println!("cargo:rustc-cfg=dock");
}

/// The Windows linker resolves every import at build time. Qt's import
/// libraries are in `OBS_MOQT_QT_LIB_DIR`; OBS ships none, so the ones for
/// `DOCK_IMPORTS` are made here from the function names.
fn link_dock_imports() {
    let qt_lib_dir = env::var("OBS_MOQT_QT_LIB_DIR")
        .expect("OBS_MOQT_QT_LIB_DIR must name the directory of Qt's import libraries");
    println!("cargo:rustc-link-search=native={qt_lib_dir}");
    for library in ["Qt6Core", "Qt6Gui", "Qt6Widgets"] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }

    let out_dir = env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let target = env::var("TARGET").expect("cargo sets TARGET");
    println!("cargo:rustc-link-search=native={out_dir}");
    for (dll, functions) in DOCK_IMPORTS {
        // A name of its own: `libobs.rs` imports from `obs.dll` as well.
        let library = format!("{dll}-dock");
        let definition = Path::new(&out_dir).join(format!("{library}.def"));
        fs::write(
            &definition,
            format!("LIBRARY {dll}.dll\nEXPORTS\n{}\n", functions.join("\n")),
        )
        .expect("write the module definition file");
        let made = cc::windows_registry::find(&target, "lib.exe")
            .expect("MSVC's lib.exe")
            .arg("/nologo")
            .arg("/machine:x64")
            .arg(format!("/def:{}", definition.display()))
            .arg(format!(
                "/out:{}",
                Path::new(&out_dir).join(format!("{library}.lib")).display()
            ))
            .status()
            .expect("run lib.exe");
        assert!(made.success(), "lib.exe failed for {dll}.dll");
        println!("cargo:rustc-link-lib=dylib={library}");
    }
}
