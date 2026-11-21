use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use moq_wasm::{
    CertificateVerification, ConnectOptions, SessionEnd, media,
    robotics::{self, ArbiterConfig, Command},
};
use serde_json::{Map, Value, json};

/// The control loop's period, at which `state` is published too.
const TICK: Duration = Duration::from_millis(50);
/// The plane is the square [-LIMIT, LIMIT]².
const LIMIT: f64 = 1.0;

#[derive(Parser)]
struct Cli {
    /// The relay: `moqt://` over raw QUIC, `https://` over WebTransport.
    #[arg(long, default_value = "moqt://127.0.0.1:4433")]
    url: String,
    /// Accept any certificate.
    #[arg(long)]
    insecure: bool,
    /// The JWT, for a relay that verifies tokens.
    #[arg(long)]
    token: Option<String>,
    /// The room, `/`-separated; the robot publishes `<room>/robot`.
    #[arg(long, default_value = "anon/robot-teleop")]
    room: String,
}

/// The point the robot is, and the velocity it moves at, in units per
/// second.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Body {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
}

impl Body {
    fn step(&mut self, dt: Duration) {
        let seconds = dt.as_secs_f64();
        self.x = (self.x + self.vx * seconds).clamp(-LIMIT, LIMIT);
        self.y = (self.y + self.vy * seconds).clamp(-LIMIT, LIMIT);
    }

    /// An applied action's `[vx, vy]`; a missing component is 0.
    fn drive(&mut self, values: &[f64]) {
        self.vx = values.first().copied().unwrap_or(0.0);
        self.vy = values.get(1).copied().unwrap_or(0.0);
    }

    fn stop(&mut self) {
        self.vx = 0.0;
        self.vy = 0.0;
    }

    fn state(&self) -> Map<String, Value> {
        let Value::Object(fields) = json!({"x": self.x, "y": self.y}) else {
            unreachable!("a json! object literal is an object");
        };
        fields
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "moq_wasm=info,robotics=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let options = ConnectOptions {
        token: cli.token.clone(),
        certificate_verification: if cli.insecure {
            CertificateVerification::Disabled
        } else {
            CertificateVerification::default()
        },
        ..ConnectOptions::default()
    };
    let session = moq_wasm::connect(&cli.url, options)
        .await
        .with_context(|| format!("connect to {}", cli.url))?;
    let mut namespace: Vec<&str> = cli.room.split('/').collect();
    namespace.push("robot");
    let broadcast = media::create_broadcast(&session, &namespace)
        .announce()
        .await?;
    let mut robot = robotics::run_robot(&session, &broadcast, ArbiterConfig::default()).await?;
    println!("robot ready in {}", cli.room);

    let mut body = Body::default();
    let mut ticks = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            command = robot.next_command() => match command {
                Some(Command::Apply { values, .. }) => body.drive(&values),
                Some(Command::Hold(_)) => body.stop(),
                None => break,
            },
            _ = ticks.tick() => {
                body.step(TICK);
                robot.publish_state(body.state()).await?;
            }
            end = session.closed() => match end {
                SessionEnd::Closed => break,
                end => anyhow::bail!("the session ended: {end:?}"),
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    session.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_moves_at_the_applied_velocity_and_stays_on_the_plane() {
        // Arrange
        let mut body = Body::default();
        body.drive(&[0.5, -4.0]);

        // Act
        body.step(Duration::from_millis(500));

        // Assert
        assert_eq!((body.x, body.y), (0.25, -LIMIT));
    }

    #[test]
    fn a_hold_stops_the_body() {
        // Arrange
        let mut body = Body::default();
        body.drive(&[1.0]);

        // Act
        body.stop();
        body.step(TICK);

        // Assert
        assert_eq!(body, Body::default());
    }
}
