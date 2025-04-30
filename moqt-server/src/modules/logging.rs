use console_subscriber::ConsoleLayer;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{self, filter::LevelFilter, fmt, EnvFilter, Layer, Registry};
pub fn init_logging(log_level: String) {
    // tokio-console用のレイヤーとフィルタ
    let console_filter = EnvFilter::builder()
        .with_default_directive("trace".parse().unwrap())
        .from_env_lossy();
    let console_layer = ConsoleLayer::builder().spawn().with_filter(console_filter);

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
        .with(stdout_layer)
        .with(file_layer)
        .init();
}
