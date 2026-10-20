//! One log file per run under `<data dir>/logs`, for users to attach to a bug
//! report: the version, which commands ran, warnings, errors and crashes.
//! Command arguments and match data are never written, and the home folder
//! and Windows user name are scrubbed from every line.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// Logs kept, counting this run's; older ones are deleted at startup.
const KEEP: usize = 20;

struct Log {
    file: File,
    path: PathBuf,
    started: Instant,
    /// `(text, replacement)` pairs, longest first.
    scrub: Vec<(String, &'static str)>,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Opens this run's log. `mode` is `shell` or the subcommand's name. Failing
/// to create the file only means this run isn't logged.
pub fn start(data_dir: &Path, mode: &str) {
    let dir = dir(data_dir);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    prune(&dir, KEEP - 1);
    let stamp = vantage_core::time::now_rfc3339().replace(':', "-");
    let path = dir.join(format!("vantage-{stamp}-{}.log", std::process::id()));
    let Ok(file) = File::options().create_new(true).append(true).open(&path) else {
        return;
    };
    let log = Log {
        file,
        path,
        started: Instant::now(),
        scrub: scrub_list(),
    };
    if let Ok(mut slot) = LOG.lock() {
        *slot = Some(log);
    }
    write(
        "INFO",
        &format!(
            "{} {} ({} {}) · {mode} · started {}",
            crate::APP_NAME,
            crate::VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH,
            vantage_core::time::now_rfc3339(),
        ),
    );
}

/// This run's log file, if one is open.
pub fn path() -> Option<PathBuf> {
    LOG.lock().ok()?.as_ref().map(|l| l.path.clone())
}

/// One entry; continuation lines are indented under it.
pub fn write(level: &str, text: &str) {
    let Ok(mut slot) = LOG.lock() else {
        return;
    };
    let Some(log) = slot.as_mut() else {
        return;
    };
    let text = scrub(text, &log.scrub);
    let secs = log.started.elapsed().as_secs_f64();
    let body = text.trim_end().replace('\n', "\n              ");
    let _ = writeln!(log.file, "[{secs:>9.3}s] {level:<5} {body}");
}

/// Logs every panic (with a backtrace) before the default hook runs. In the
/// shell the default hook's message would land on the alternate screen and
/// vanish, so it only runs outside it.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        write(
            "PANIC",
            &format!("thread '{}' {info}\n{backtrace}", thread.name().unwrap_or("worker")),
        );
        if !crate::out::captured() {
            default(info);
        }
    }));
}

/// " · details in <log>", for messages that ask the user to report something.
pub fn hint() -> String {
    path().map_or_else(String::new, |p| format!(" · details in {}", p.display()))
}

/// Deletes all but the newest `keep` logs.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == "log")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("vantage-"))
        })
        .collect();
    // The names start with a UTC timestamp, so they sort oldest first.
    logs.sort();
    let extra = logs.len().saturating_sub(keep);
    for p in &logs[..extra] {
        let _ = fs::remove_file(p);
    }
}

fn scrub(text: &str, list: &[(String, &'static str)]) -> String {
    let mut text = text.to_owned();
    for (from, to) in list {
        text = text.replace(from.as_str(), to);
    }
    text
}

/// The home folder becomes `~` and the user name `<user>`, in both slash styles.
fn scrub_list() -> Vec<(String, &'static str)> {
    let mut list = Vec::new();
    for var in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(var).map(|h| h.to_string_lossy().into_owned())
            && home.len() > 3
        {
            list.push((home.replace('/', "\\"), "~"));
            list.push((home.replace('\\', "/"), "~"));
        }
    }
    for var in ["USERNAME", "USER"] {
        // A one- or two-letter name would scrub ordinary words.
        if let Ok(name) = std::env::var(var)
            && name.len() >= 3
        {
            list.push((name, "<user>"));
        }
    }
    list.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    list.dedup();
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_home_and_user_name() {
        let list = vec![
            (r"C:\Users\pat".to_owned(), "~"),
            ("C:/Users/pat".to_owned(), "~"),
            ("pat".to_owned(), "<user>"),
        ];
        assert_eq!(
            scrub(r"can't open C:\Users\pat\Videos and C:/Users/pat/x (pat)", &list),
            r"can't open ~\Videos and ~/x (<user>)"
        );
    }

    #[test]
    fn prune_keeps_the_newest() {
        let dir = std::env::temp_dir().join(format!("vantage-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for day in 1..=5 {
            File::create(dir.join(format!("vantage-2026-10-0{day}T00-00-00Z-1.log"))).unwrap();
        }
        File::create(dir.join("notes.txt")).unwrap();
        prune(&dir, 2);
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "notes.txt",
                "vantage-2026-10-04T00-00-00Z-1.log",
                "vantage-2026-10-05T00-00-00Z-1.log"
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
