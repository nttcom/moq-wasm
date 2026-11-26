use gstreamer as gst;
use tracing_subscriber::EnvFilter;

mod media_input;
mod sink;

fn plugin_init(plugin: &gst::Plugin) -> Result<(), gst::glib::BoolError> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();
    sink::register(plugin)
}

gst::plugin_define!(
    moqt,
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    env!("CARGO_PKG_VERSION"),
    "MIT/X11",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY")
);
