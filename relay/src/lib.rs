mod config;
mod logging;
pub use config::{AuthConfig, RelayConfig};
pub use logging::{LoggingGuards, init_logging};
pub use modules::auth::token_claims::ClaimPolicy;
pub mod modules;
mod relay_server;

pub use relay_server::server::RelayServer;
