use console_subscriber::ConsoleLayer;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{self, filter::LevelFilter, fmt, EnvFilter, Layer, Registry};
pub fn init_logging(log_level: String) {
    // tokio-console用のレイヤーとフィルタ(For Development)
    let console_filter = EnvFilter::new("tokio::task=trace");
    let console_layer = ConsoleLayer::builder()
        .retention(std::time::Duration::from_secs(3600)) // Default: 3600
        .spawn()
        .with_filter(console_filter);
    // tokio-console用のレイヤーとフィルタ(For Debug)
    // let debug_console_filter =
    //     EnvFilter::new("tokio::task=trace,tokio::sync=trace,tokio::timer=trace");
    // let debug_console_layer = ConsoleLayer::builder()
    //     .event_buffer_capacity(1024 * 250) // Default: 102400
    //     .client_buffer_capacity(1024 * 7) // Default: 1024
    //     .retention(std::time::Duration::from_secs(600)) // Default: 3600
    //     .spawn()
    //     .with_filter(debug_console_filter);

    // 標準出力用のレイヤーとフィルタ
    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(log_level.parse().unwrap())
                .from_env_lossy(),
        );

    // ログファイル用のレイヤーとフィルタ
    let file_layer = fmt::layer()
        .with_writer(rolling::hourly("./log", "output"))
        // Multi Writer with_ansi option doesn't work https://github.com/tokio-rs/tracing/issues/3116
        // .with_ansi(false)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::DEBUG.into())
                .from_env_lossy(),
        );

    Registry::default()
        .with(console_layer)
        // .with(debug_console_layer)
        .with(stdout_layer)
        .with(file_layer)
        .init();
}
