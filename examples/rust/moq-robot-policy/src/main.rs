use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use moq_wasm::{
    CertificateVerification, ConnectOptions, SessionEnd, SubscribeOptions, media,
    robotics::{self, Driver, State, decode_object},
};

/// The duration each action is meant for, the robot's `state` period.
const DT_MS: u64 = 50;
/// The fastest the policy drives along each axis, in units per second.
const MAX_SPEED: f64 = 0.5;
/// The robot's `state` priority and the action age it accepts
/// (docs/architecture/robotics/architecture.md, Priority and delivery).
const STATE_PRIORITY: u8 = 32;
const MAX_ACTION_AGE: Duration = Duration::from_millis(300);
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(1);

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
    /// The room, `/`-separated; the policy publishes `<room>/bots/policy`.
    #[arg(long, default_value = "anon/robot-teleop")]
    room: String,
    /// The point the policy drives the robot to, `x,y`.
    #[arg(long, default_value = "0.5,0.5", value_parser = parse_point)]
    target: (f64, f64),
}

fn parse_point(text: &str) -> Result<(f64, f64), String> {
    let (x, y) = text.split_once(',').ok_or("expected x,y")?;
    let parse = |value: &str| {
        value
            .trim()
            .parse::<f64>()
            .map_err(|error| error.to_string())
    };
    Ok((parse(x)?, parse(y)?))
}

/// The velocity toward `target` from `(x, y)`, proportional to the distance
/// and at most `MAX_SPEED` along each axis.
fn velocity_toward(target: (f64, f64), x: f64, y: f64) -> [f64; 2] {
    let speed = |distance: f64| (2.0 * distance).clamp(-MAX_SPEED, MAX_SPEED);
    [speed(target.0 - x), speed(target.1 - y)]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "moq_wasm=info".into()),
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
    let room: Vec<&str> = cli.room.split('/').collect();
    let policy: Vec<&str> = room.iter().copied().chain(["bots", "policy"]).collect();
    let robot: Vec<&str> = room.iter().copied().chain(["robot"]).collect();
    let broadcast = media::create_broadcast(&session, &policy)
        .announce()
        .await?;
    let driver = robotics::offer_actions(&broadcast).await?;
    println!("policy ready in {}", cli.room);

    tokio::select! {
        result = drive(&session, &robot, &driver, cli.target) => result?,
        end = session.closed() => match end {
            SessionEnd::Closed => {}
            end => anyhow::bail!("the session ended: {end:?}"),
        },
        _ = tokio::signal::ctrl_c() => {}
    }
    session.close().await;
    Ok(())
}

/// Answers each `state` of the robot with an action toward `target`,
/// subscribing again a second after the robot leaves or before it comes.
async fn drive(
    session: &moq_wasm::Session,
    robot: &[&str],
    driver: &Driver,
    target: (f64, f64),
) -> anyhow::Result<()> {
    let options = SubscribeOptions {
        priority: STATE_PRIORITY,
        delivery_timeout: Some(MAX_ACTION_AGE),
        ..SubscribeOptions::default()
    };
    loop {
        if let Ok(subscription) = session.subscribe(robot, "state", options).await {
            while let Ok(Some(object)) = subscription.next().await {
                let Some(state) = decode_object::<State>(&object.payload)
                    .ok()
                    .and_then(|mut records| records.pop())
                else {
                    continue;
                };
                let position = |axis: &str| state.data.fields.get(axis).and_then(|v| v.as_f64());
                let (Some(x), Some(y)) = (position("x"), position("y")) else {
                    continue;
                };
                let velocity = velocity_toward(target, x, y);
                driver.send(state.t, DT_MS, vec![velocity.to_vec()]).await?;
            }
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_velocity_points_at_the_target_and_is_capped() {
        // Act / Assert
        assert_eq!(velocity_toward((0.5, 0.5), 0.375, 1.0), [0.25, -0.5]);
        assert_eq!(velocity_toward((0.5, 0.5), 0.5, 0.5), [0.0, 0.0]);
    }

    #[test]
    fn a_target_is_read_as_x_comma_y() {
        // Act / Assert
        assert_eq!(parse_point("0.5, -0.25"), Ok((0.5, -0.25)));
        assert!(parse_point("0.5").is_err());
    }
}
