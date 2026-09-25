use std::{
    collections::HashMap,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use media_publisher::{MoqtManager, StreamRecord};
use moqt::TransportStats;
use tokio::task::JoinHandle;

const LABEL_WIDTH: usize = 40;
const BAR_WIDTH: usize = 30;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const CLEAR_SCREEN: &str = "\x1b[2J";
const CURSOR_HOME: &str = "\x1b[H";
const CLEAR_BELOW_CURSOR: &str = "\x1b[J";
const CLEAR_TO_LINE_END: &str = "\x1b[K";

/// Relay connections whose QUIC path statistics the panel shows; each
/// connection stays registered for as long as its `Registration` lives.
#[derive(Clone, Default)]
pub struct ConnectionRegistry {
    connections: Arc<Mutex<Vec<RegisteredConnection>>>,
    next_id: Arc<AtomicU64>,
}

struct RegisteredConnection {
    id: u64,
    label: String,
    moqt: MoqtManager,
}

struct ConnectionSnapshot {
    id: u64,
    label: String,
    stats: Option<TransportStats>,
    streams: Vec<StreamRecord>,
}

pub struct Registration {
    registry: ConnectionRegistry,
    id: u64,
}

impl ConnectionRegistry {
    pub fn register(&self, label: String, moqt: &MoqtManager) -> Registration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.connections.lock().unwrap().push(RegisteredConnection {
            id,
            label,
            moqt: moqt.clone(),
        });
        Registration {
            registry: self.clone(),
            id,
        }
    }

    fn snapshot(&self) -> Vec<ConnectionSnapshot> {
        self.connections
            .lock()
            .unwrap()
            .iter()
            .map(|c| ConnectionSnapshot {
                id: c.id,
                label: c.label.clone(),
                stats: c.moqt.transport_stats(),
                streams: c.moqt.streams(),
            })
            .collect()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry
            .connections
            .lock()
            .unwrap()
            .retain(|c| c.id != self.id);
    }
}

/// Redraws the statistics of every registered connection from the top of the
/// screen once a second, so wrapped lines never leave stale rows behind; logs
/// must go to stderr while it runs.
pub struct StatsPanel {
    _task: JoinHandle<()>,
}

impl StatsPanel {
    pub fn run(registry: ConnectionRegistry) -> Self {
        Self {
            _task: tokio::spawn(async move {
                let mut previous: HashMap<u64, TransportStats> = HashMap::new();
                let mut interval = tokio::time::interval(REFRESH_INTERVAL);
                let _ = write!(std::io::stdout(), "{CLEAR_SCREEN}");
                loop {
                    interval.tick().await;
                    let rows = registry
                        .snapshot()
                        .into_iter()
                        .map(|connection| {
                            let row = StatsRow {
                                label: connection.label,
                                stats: connection.stats,
                                previous: previous.get(&connection.id).copied(),
                                streams: connection.streams,
                            };
                            if let Some(stats) = connection.stats {
                                previous.insert(connection.id, stats);
                            }
                            row
                        })
                        .collect::<Vec<_>>();
                    let table = render_table(&rows);
                    let mut stdout = std::io::stdout().lock();
                    let _ = write!(stdout, "{CURSOR_HOME}{table}{CLEAR_BELOW_CURSOR}");
                    let _ = stdout.flush();
                }
            }),
        }
    }
}

struct StatsRow {
    label: String,
    stats: Option<TransportStats>,
    previous: Option<TransportStats>,
    streams: Vec<StreamRecord>,
}

const HEADER: &str = "connection                                  rtt     cwnd      sent  lost  cong_ev  stream_blocked  data_blocked  max_stream_data";

fn render_table(rows: &[StatsRow]) -> String {
    let mut out = format!(
        "MoQT transport statistics ({} connections){CLEAR_TO_LINE_END}\n{HEADER}{CLEAR_TO_LINE_END}\n",
        rows.len()
    );
    for row in rows {
        out.push_str(&render_row(row));
        out.push_str(CLEAR_TO_LINE_END);
        out.push('\n');
        for line in render_streams(&row.streams) {
            out.push_str(&line);
            out.push_str(CLEAR_TO_LINE_END);
            out.push('\n');
        }
    }
    out
}

fn render_streams(streams: &[StreamRecord]) -> Vec<String> {
    let open = streams.iter().filter(|s| s.is_open()).count();
    let mut lines = vec![format!(
        "    streams: {open} open, {} finished within 5s",
        streams.len() - open
    )];
    let widest = streams.iter().map(|s| s.bytes).max().unwrap_or(0).max(1);
    let now = Instant::now();
    let mut ordered: Vec<&StreamRecord> = streams.iter().collect();
    ordered.sort_by(|a, b| {
        a.track_name
            .cmp(&b.track_name)
            .then(a.group_id.cmp(&b.group_id))
    });
    for stream in ordered {
        let state = match stream.finished_at {
            None => format!(
                "open {:>5.1}s",
                now.duration_since(stream.opened_at).as_secs_f64()
            ),
            Some(finished_at) => format!(
                "done {:>5.1}s",
                finished_at.duration_since(stream.opened_at).as_secs_f64()
            ),
        };
        let filled = (stream.bytes * BAR_WIDTH as u64 / widest) as usize;
        lines.push(format!(
            "    {:<12} g{:<16} {state}  {:>4} obj  {:>8.1} KB  {}{}",
            truncate(&stream.track_name, 12),
            stream.group_id,
            stream.objects,
            stream.bytes as f64 / 1024.0,
            "█".repeat(filled),
            "·".repeat(BAR_WIDTH - filled),
        ));
    }
    lines
}

fn render_row(row: &StatsRow) -> String {
    let Some(stats) = row.stats else {
        return format!(
            "{:<LABEL_WIDTH$}  connecting to the relay",
            truncate(&row.label, LABEL_WIDTH)
        );
    };
    let previous = row.previous.unwrap_or(stats);
    format!(
        "{:<LABEL_WIDTH$}  {:>5.1}ms  {:>5}K  {:>8}  {:>4}  {:>7}  {:>14}  {:>12}  {:>15}",
        truncate(&row.label, LABEL_WIDTH),
        stats.rtt.as_secs_f64() * 1000.0,
        stats.cwnd / 1024,
        counter(stats.sent_packets, previous.sent_packets),
        counter(stats.lost_packets, previous.lost_packets),
        counter(stats.congestion_events, previous.congestion_events),
        counter(
            stats.sent_stream_data_blocked,
            previous.sent_stream_data_blocked
        ),
        counter(stats.sent_data_blocked, previous.sent_data_blocked),
        counter(
            stats.received_max_stream_data,
            previous.received_max_stream_data
        ),
    )
}

fn counter(current: u64, previous: u64) -> String {
    match current.saturating_sub(previous) {
        0 => current.to_string(),
        delta => format!("{current}(+{delta})"),
    }
}

fn truncate(label: &str, width: usize) -> String {
    if label.chars().count() <= width {
        return label.to_string();
    }
    label
        .chars()
        .take(width - 1)
        .chain(std::iter::once('…'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(lost_packets: u64, sent_stream_data_blocked: u64) -> TransportStats {
        TransportStats {
            rtt: Duration::from_micros(12_345),
            cwnd: 2 * 1024 * 1024,
            sent_packets: 100,
            lost_packets,
            congestion_events: 0,
            sent_stream_data_blocked,
            sent_data_blocked: 0,
            received_max_stream_data: 7,
        }
    }

    #[test]
    fn row_shows_the_increase_since_the_previous_tick_next_to_each_counter() {
        // Arrange
        let row = StatsRow {
            label: "srt 10.0.0.1:5000 live/test".to_string(),
            stats: Some(stats(5, 3)),
            previous: Some(stats(2, 3)),
            streams: Vec::new(),
        };

        // Act
        let line = render_row(&row);

        // Assert
        assert!(line.contains("12.3ms"), "{line}");
        assert!(line.contains(" 2048K"), "{line}");
        assert!(line.contains("5(+3)"), "{line}");
        assert!(line.contains("  3  "), "{line}");
    }

    #[test]
    fn row_without_a_session_says_it_is_connecting() {
        // Arrange
        let row = StatsRow {
            label: "rtmp 10.0.0.2:1935".to_string(),
            stats: None,
            previous: None,
            streams: Vec::new(),
        };

        // Act / Assert
        assert!(render_row(&row).ends_with("connecting to the relay"));
    }

    #[test]
    fn stream_lines_scale_the_bar_to_the_largest_stream() {
        // Arrange
        let opened_at = Instant::now();
        let streams = vec![
            StreamRecord {
                track_name: "video".to_string(),
                group_id: 7,
                opened_at,
                bytes: 3000,
                objects: 30,
                finished_at: None,
            },
            StreamRecord {
                track_name: "audio".to_string(),
                group_id: 7,
                opened_at,
                bytes: 1000,
                objects: 40,
                finished_at: Some(opened_at),
            },
        ];

        // Act
        let lines = render_streams(&streams);

        // Assert
        assert_eq!(lines[0], "    streams: 1 open, 1 finished within 5s");
        assert!(lines[1].starts_with("    audio        g7"), "{}", lines[1]);
        assert!(lines[1].contains("done"), "{}", lines[1]);
        assert!(lines[1].contains(&"█".repeat(10)), "{}", lines[1]);
        assert!(!lines[1].contains(&"█".repeat(11)), "{}", lines[1]);
        assert!(lines[2].contains("open"), "{}", lines[2]);
        assert!(lines[2].ends_with(&"█".repeat(30)), "{}", lines[2]);
    }

    #[test]
    fn table_has_one_line_per_connection_after_the_two_header_lines() {
        // Arrange
        let rows = vec![
            StatsRow {
                label: "a".to_string(),
                stats: Some(stats(0, 0)),
                previous: None,
                streams: Vec::new(),
            },
            StatsRow {
                label: "b".to_string(),
                stats: None,
                previous: None,
                streams: Vec::new(),
            },
        ];

        // Act
        let table = render_table(&rows);

        // Assert: two header lines, then one connection line and one stream summary line each
        assert_eq!(table.lines().count(), 6);
    }

    #[test]
    fn dropping_the_registration_removes_the_connection() {
        // Arrange
        let registry = ConnectionRegistry::default();
        let moqt = MoqtManager::new(None);
        let registration = registry.register("a".to_string(), &moqt);

        // Act
        let while_registered = registry.snapshot().len();
        drop(registration);

        // Assert
        assert_eq!(while_registered, 1);
        assert!(registry.snapshot().is_empty());
    }
}
