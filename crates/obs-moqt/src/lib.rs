//! OBS Studio plugin that registers the `moqt_output` output and the
//! `moqt_service` service. Only the modules without libobs calls are built
//! for unit tests, because libobs symbols exist only inside OBS.

mod media_input;
mod publish_settings;

#[cfg(not(test))]
mod libobs;
#[cfg(not(test))]
mod output;
#[cfg(not(test))]
mod service;

#[cfg(not(test))]
mod module {
    use std::{
        ptr,
        sync::atomic::{AtomicPtr, Ordering},
    };

    use tracing_subscriber::EnvFilter;

    use crate::{
        libobs::{LIBOBS_API_VER, ObsModule},
        output, service,
    };

    static MODULE: AtomicPtr<ObsModule> = AtomicPtr::new(ptr::null_mut());

    #[cfg(dock)]
    unsafe extern "C" {
        /// `dock.cpp`: adds the MoQT dock to OBS's main window.
        fn obs_moqt_add_dock();
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn obs_module_set_pointer(module: *mut ObsModule) {
        MODULE.store(module, Ordering::Relaxed);
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn obs_current_module() -> *mut ObsModule {
        MODULE.load(Ordering::Relaxed)
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn obs_module_ver() -> u32 {
        LIBOBS_API_VER
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn obs_module_load() -> bool {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .with_writer(std::io::stderr)
            .try_init();
        output::register();
        service::register();
        // SAFETY: OBS loads modules on its UI thread, after the main window
        // the dock is added to exists.
        #[cfg(dock)]
        unsafe {
            obs_moqt_add_dock()
        };
        true
    }
}
