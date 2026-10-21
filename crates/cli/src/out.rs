//! Where command output goes: stdout and stderr, or the interactive shell's
//! transcript while it runs a command. It's shellkit's (`shellkit::out`);
//! warnings and errors also go to the session log (`shellkit::log`).

pub use shellkit::out::*;

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
