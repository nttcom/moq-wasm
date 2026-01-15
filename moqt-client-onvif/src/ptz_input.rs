use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use std::thread;
use tokio::sync::mpsc;

#[derive(Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

pub enum Command {
    Move(Direction),
    Center,
    Quit,
}

pub fn spawn_input_loop(tx: mpsc::Sender<Command>) {
    thread::spawn(move || input_loop(tx));
}

fn input_loop(tx: mpsc::Sender<Command>) {
    let _raw_mode = RawModeGuard::new();
    loop {
        match event::read() {
            Ok(Event::Key(key)) => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let _ = tx.blocking_send(Command::Quit);
                    break;
                }
                KeyCode::Left => send_move(&tx, key.kind, Direction::Left),
                KeyCode::Right => send_move(&tx, key.kind, Direction::Right),
                KeyCode::Up => send_move(&tx, key.kind, Direction::Up),
                KeyCode::Down => send_move(&tx, key.kind, Direction::Down),
                KeyCode::Char(' ') | KeyCode::Char('s') => {
                    let _ = tx.blocking_send(Command::Center);
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    let _ = tx.blocking_send(Command::Quit);
                    break;
                }
                _ => {}
            },
            Err(_) => break,
            _ => {}
        }
    }
}

fn send_move(tx: &mpsc::Sender<Command>, kind: KeyEventKind, dir: Direction) {
    if matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        let _ = tx.blocking_send(Command::Move(dir));
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Self {
        let _ = terminal::enable_raw_mode();
        Self
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}
