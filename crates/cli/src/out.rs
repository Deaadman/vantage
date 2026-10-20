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
}

static SINK: Mutex<Option<Sender<(Kind, String)>>> = Mutex::new(None);

/// Send output to `tx` instead of the terminal; `None` restores the terminal.
pub fn capture(tx: Option<Sender<(Kind, String)>>) {
    if let Ok(mut sink) = SINK.lock() {
        *sink = tx;
    }
}

pub fn emit(kind: Kind, text: String) {
    let sink = SINK.lock().ok().and_then(|s| s.clone());
    match sink {
        Some(tx) => {
            for line in text.split('\n') {
                let _ = tx.send((kind, line.trim_end_matches('\r').to_owned()));
            }
        }
        None if kind == Kind::Out || kind == Kind::Dim || kind == Kind::Bold => println!("{text}"),
        None => eprintln!("{text}"),
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
