//! Where command output goes: stdout and stderr, or the interactive shell's
//! transcript while it runs a command.

use std::sync::Mutex;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Ordinary output (stdout).
    Out,
    /// Warnings and notes (stderr).
    Warn,
    /// A failed command.
    Error,
    /// Hints, in grey.
    Dim,
    /// Section titles.
    Bold,
    /// Something that worked, in green.
    Good,
    /// Any colour, bold or not (ranks, habits, accents).
    Paint(crossterm::style::Color, bool),
}

/// One transcript line: pieces in their own styles.
pub type Line = Vec<(Kind, String)>;

static SINK: Mutex<Option<Sender<Line>>> = Mutex::new(None);

/// Send output to `tx` instead of the terminal; `None` restores the terminal.
pub fn capture(tx: Option<Sender<Line>>) {
    if let Ok(mut sink) = SINK.lock() {
        *sink = tx;
    }
}

/// Whether output is going to the shell's transcript.
pub fn captured() -> bool {
    SINK.lock().is_ok_and(|s| s.is_some())
}

/// Shows `text`; warnings and errors also go to the session log.
pub fn emit(kind: Kind, text: String) {
    match kind {
        Kind::Warn => crate::session_log::write("WARN", &text),
        Kind::Error => crate::session_log::write("ERROR", &text),
        _ => {}
    }
    show(kind, text);
}

/// Shows `text` without logging it (the user's own typing, echoed back).
pub fn show(kind: Kind, text: String) {
    let sink = SINK.lock().ok().and_then(|s| s.clone());
    match sink {
        Some(tx) => {
            for line in text.split('\n') {
                let _ = tx.send(vec![(kind, line.trim_end_matches('\r').to_owned())]);
            }
        }
        None if matches!(kind, Kind::Out | Kind::Dim | Kind::Bold | Kind::Good | Kind::Paint(..)) => println!("{text}"),
        None => eprintln!("{text}"),
    }
}

/// One line built from styled pieces; plain text on stdout outside the shell.
pub fn row(pieces: Line) {
    let sink = SINK.lock().ok().and_then(|s| s.clone());
    match sink {
        Some(tx) => {
            let _ = tx.send(pieces);
        }
        None => println!("{}", pieces.into_iter().map(|(_, t)| t).collect::<String>()),
    }
}

/// `println!`, or a transcript line in the shell.
macro_rules! outln {
    () => {
        $crate::out::emit($crate::out::Kind::Out, String::new())
    };
    ($($t:tt)*) => {
        $crate::out::emit($crate::out::Kind::Out, format!($($t)*))
    };
}

/// `eprintln!`, or a warning line in the shell.
macro_rules! errln {
    ($($t:tt)*) => {
        $crate::out::emit($crate::out::Kind::Warn, format!($($t)*))
    };
}
