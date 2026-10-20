//! `valreview` with no command: scan, show recent matches, then read
//! `/commands` with a suggestion menu until the user quits.

use std::io::{self, Write};
use std::path::Path;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, queue};
use valreview_core::config::Config;
use valreview_core::paths;
use valreview_core::store::Store;
use valreview_core::vrfkit::Vrfkit;

use crate::Cli;
use crate::editor::{Action, CommandInfo, Context, Editor, FlagInfo, MatchInfo};

const PROMPT: &str = "> ";
const MENU_ROWS: usize = 8;
const HISTORY_LINES: usize = 500;
const RECENT_MATCHES: usize = 5;

/// Shell-only commands, after the clap ones.
const BUILTINS: [(&str, &str); 3] = [
    ("help", "Show the commands and keys"),
    ("clear", "Clear the screen"),
    ("exit", "Quit valreview"),
];

pub fn run(data_dir: &Path) -> Result<()> {
    println!(
        "{}valreview {}{} · facts from your own Valorant replays",
        SetAttribute(Attribute::Bold),
        env!("CARGO_PKG_VERSION"),
        SetAttribute(Attribute::Reset)
    );
    crate::startup_check(data_dir);
    launch_scan(data_dir);
    let mut ctx = Context {
        commands: commands(),
        matches: matches(data_dir),
    };
    recent(&ctx);
    hint("Type / for commands · Tab completes · ↑↓ select · Esc hides · Ctrl+C twice quits");

    let history_path = data_dir.join("history.txt");
    let mut editor = Editor::new(load_history(&history_path));
    loop {
        println!();
        let Some(line) = read_line(&mut editor, &ctx)? else {
            break;
        };
        if line.is_empty() {
            continue;
        }
        save_history(&history_path, &line);
        if !execute_line(&line, data_dir) {
            break;
        }
        ctx.matches = matches(data_dir);
    }
    Ok(())
}

/// Scan on launch when vrfkit and the replay folder are set; otherwise say how.
fn launch_scan(data_dir: &Path) {
    let cfg = Config::load(data_dir).unwrap_or_default();
    if let Err(e) = Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()) {
        println!("\nNot set up yet: {e:#}");
        hint("Run /init --vrfkit \"C:\\path\\to\\vrfkit.exe\", then /sync and /scan.");
        return;
    }
    if !paths::catalog_path(data_dir).is_file() {
        println!(
            "
Fetching agent, map and weapon names from valorant-api.com (first run only)"
        );
        if let Err(e) = crate::run_command(data_dir, crate::Command::Sync) {
            eprintln!("sync failed, using built-in names: {e:#}");
        }
    }
    match cfg.demos_dir() {
        Some(d) if d.is_dir() => {
            println!();
            if let Err(e) = crate::scan_cmd(data_dir, None, false, false, false) {
                eprintln!("scan failed: {e:#}");
            }
            if cfg.me.is_none() {
                detect_me(data_dir);
            }
        }
        Some(d) => {
            println!("\nNo replays yet: {} does not exist.", d.display());
            hint("Download a match in the client's career tab, then run /scan.");
        }
        None => hint("No replay folder set: /init --demos-dir <folder>"),
    }
}

/// Set `me` once exactly one account is in every scanned match (2+), as `init` would.
fn detect_me(data_dir: &Path) {
    let Ok(store) = Store::open(&paths::db_path(data_dir)) else {
        return;
    };
    let Ok((n, subjects)) = store.subjects_in_every_match() else {
        return;
    };
    if n < 2 || subjects.len() != 1 {
        return;
    }
    let Ok(mut cfg) = Config::load(data_dir) else {
        return;
    };
    cfg.me = Some(subjects[0].clone());
    if cfg.save(data_dir).is_ok() {
        println!(
            "You are {} (the only account in all {n} matches); change it with /init --me",
            subjects[0]
        );
    }
}

fn recent(ctx: &Context) {
    if ctx.matches.is_empty() {
        return;
    }
    println!(
        "\n{}Recent matches{}",
        SetAttribute(Attribute::Bold),
        SetAttribute(Attribute::Reset)
    );
    for m in ctx.matches.iter().take(RECENT_MATCHES) {
        println!("  {}  {}", &m.id[..m.id.len().min(8)], m.label);
    }
    if ctx.matches.len() > RECENT_MATCHES {
        hint(&format!("…and {} more (/matches)", ctx.matches.len() - RECENT_MATCHES));
    }
    hint("/packet reviews the newest; /packet <space> picks another");
}

fn hint(s: &str) {
    println!("{}{s}{}", SetForegroundColor(Color::DarkGrey), ResetColor);
}

/// The clap subcommands and their long flags, then the built-ins.
fn commands() -> Vec<CommandInfo> {
    let mut out: Vec<CommandInfo> = Cli::command()
        .get_subcommands()
        .map(|sub| CommandInfo {
            name: sub.get_name().to_owned(),
            about: sub.get_about().map(|s| s.to_string()).unwrap_or_default(),
            flags: sub
                .get_arguments()
                .filter(|a| !a.is_global_set())
                .filter_map(|a| {
                    let long = a.get_long()?;
                    Some(FlagInfo {
                        long: format!("--{long}"),
                        help: a.get_help().map(|s| s.to_string()).unwrap_or_default(),
                        value: a.get_action().takes_values().then(|| {
                            a.get_value_names()
                                .and_then(|v| v.first())
                                .map_or_else(|| long.to_uppercase(), |v| v.to_string())
                        }),
                    })
                })
                .collect(),
        })
        .collect();
    out.extend(BUILTINS.iter().map(|(name, about)| CommandInfo {
        name: (*name).into(),
        about: (*about).into(),
        flags: vec![],
    }));
    out
}

fn matches(data_dir: &Path) -> Vec<MatchInfo> {
    let db = paths::db_path(data_dir);
    if !db.is_file() {
        return vec![];
    }
    let Ok(rows) = Store::open(&db).and_then(|s| s.matches()) else {
        return vec![];
    };
    rows.into_iter()
        .map(|(id, played, map)| MatchInfo {
            id,
            label: format!(
                "{}  {}",
                played.as_deref().map(short_time).unwrap_or_else(|| "?".into()),
                map.as_deref().map(short_map).unwrap_or("?")
            ),
        })
        .collect()
}

/// `2026-10-19T21:04:11Z` -> `2026-10-19 21:04`.
fn short_time(t: &str) -> String {
    t.get(..16).unwrap_or(t).replacen('T', " ", 1)
}

/// A map name, or the last part of an uncatalogued map path:
/// `/Game/Maps/Ascent/Ascent` -> `Ascent`.
fn short_map(m: &str) -> &str {
    m.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(m)
}

/// Run one line. False when the shell should quit.
fn execute_line(line: &str, data_dir: &Path) -> bool {
    let mut words = split_words(line);
    let Some(first) = words.first_mut() else {
        return true;
    };
    *first = first.trim_start_matches('/').to_lowercase();
    match first.as_str() {
        "exit" | "quit" => return false,
        "clear" => {
            let _ = execute!(
                io::stdout(),
                Clear(ClearType::All),
                Clear(ClearType::Purge),
                crossterm::cursor::MoveTo(0, 0)
            );
            return true;
        }
        "help" => {
            help();
            return true;
        }
        _ => {}
    }
    let cli = match Cli::try_parse_from(std::iter::once("valreview".to_owned()).chain(words)) {
        Ok(cli) => cli,
        Err(e) => {
            // Also how `--help` prints.
            let _ = e.print();
            return true;
        }
    };
    let Some(command) = cli.command else {
        return true;
    };
    let dir = cli.data_dir.unwrap_or_else(|| data_dir.to_path_buf());
    if let Err(e) = crate::run_command(&dir, command) {
        eprintln!("{}error:{} {e:#}", SetForegroundColor(Color::Red), ResetColor);
    }
    true
}

fn help() {
    for c in commands() {
        println!("  {:<10} {}", format!("/{}", c.name), c.about);
    }
    println!();
    hint("Keys: Tab or → completes, ↑↓ move in the menu (or through history), Enter runs,");
    hint("Esc hides the menu, Ctrl+U clears to the start, Ctrl+W deletes a word, Ctrl+L clears the screen.");
    hint("Every command takes --help, e.g. /packet --help.");
}

/// Split on whitespace; double quotes group. Backslashes stay literal for Windows paths.
fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    words.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(cur);
    }
    words
}

fn load_history(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn save_history(path: &Path, line: &str) {
    let mut lines = load_history(path);
    if lines.last().map(String::as_str) == Some(line) {
        return;
    }
    lines.push(line.to_owned());
    let skip = lines.len().saturating_sub(HISTORY_LINES);
    let _ = std::fs::write(path, lines[skip..].join("\n") + "\n");
}

/// Leaves raw mode however `read_line` exits.
struct RawMode;

impl RawMode {
    fn on() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let _ = execute!(io::stdout(), event::EnableBracketedPaste);
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), event::DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
    }
}

/// One line from the user, or None to quit.
fn read_line(editor: &mut Editor, ctx: &Context) -> io::Result<Option<String>> {
    let _raw = RawMode::on()?;
    let mut out = io::stdout();
    loop {
        draw(&mut out, editor, ctx, true)?;
        let action = match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => editor.handle(k, ctx),
            Event::Paste(s) => {
                editor.insert_str(&s);
                Action::None
            }
            _ => Action::None,
        };
        match action {
            Action::None => {}
            Action::ClearScreen => {
                execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
            }
            Action::Exit => {
                draw(&mut out, editor, ctx, false)?;
                execute!(out, Print("\r\n"))?;
                return Ok(None);
            }
            Action::Submit(line) => {
                // Leave the submitted line on screen without the menu.
                queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
                prompt(&mut out)?;
                execute!(out, Print(&line), Print("\r\n"))?;
                return Ok(Some(line));
            }
        }
    }
}

fn prompt(out: &mut impl Write) -> io::Result<()> {
    queue!(
        out,
        SetForegroundColor(Color::Magenta),
        SetAttribute(Attribute::Bold),
        Print(PROMPT),
        SetAttribute(Attribute::Reset),
        ResetColor
    )
}

/// Redraw the prompt line and the menu under it, leaving the cursor on the
/// prompt line. Everything is clipped to one terminal row per line.
fn draw(out: &mut impl Write, editor: &Editor, ctx: &Context, with_menu: bool) -> io::Result<()> {
    let width = terminal::size().map_or(80, |(w, _)| w as usize).max(20);
    let room = width - PROMPT.len() - 1;
    let line: Vec<char> = editor.line().chars().collect();
    let cursor = editor.cursor();
    let start = cursor.saturating_sub(room);
    let shown: String = line[start..line.len().min(start + room)].iter().collect();

    queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
    prompt(out)?;
    // The command name in colour once it is a real command.
    let name_end = shown.find(char::is_whitespace).unwrap_or(shown.len());
    let name = shown[..name_end].trim_start_matches('/');
    if start == 0 && shown.starts_with('/') && ctx.commands.iter().any(|c| c.name == name) {
        queue!(
            out,
            SetForegroundColor(Color::Cyan),
            Print(&shown[..name_end]),
            ResetColor,
            Print(&shown[name_end..])
        )?;
    } else {
        queue!(out, Print(&shown))?;
    }
    let ghost = if line.is_empty() {
        Some("Type / for commands".to_owned())
    } else {
        editor.ghost(ctx)
    };
    if let Some(g) = ghost {
        let fits: String = g.chars().take(room.saturating_sub(shown.chars().count())).collect();
        queue!(out, SetForegroundColor(Color::DarkGrey), Print(fits), ResetColor)?;
    }

    let mut rows = 0;
    if with_menu {
        let s = editor.suggestions(ctx);
        let n = s.items.len();
        let sel = editor.selected(n);
        let first = sel.saturating_sub(MENU_ROWS - 1).min(n.saturating_sub(MENU_ROWS));
        let label_w = s
            .items
            .iter()
            .map(|i| i.label.chars().count())
            .max()
            .unwrap_or(0)
            .min(30);
        for (i, item) in s.items.iter().enumerate().skip(first).take(MENU_ROWS) {
            let label: String = item.label.chars().take(label_w).collect();
            let row = format!("  {label:<label_w$}  ");
            let desc: String = item
                .description
                .chars()
                .take(width.saturating_sub(row.chars().count() + 1))
                .collect();
            queue!(out, Print("\r\n"))?;
            if i == sel {
                queue!(
                    out,
                    SetForegroundColor(Color::Cyan),
                    Print(row),
                    Print(desc),
                    ResetColor
                )?;
            } else {
                queue!(
                    out,
                    Print(row),
                    SetForegroundColor(Color::DarkGrey),
                    Print(desc),
                    ResetColor
                )?;
            }
            rows += 1;
        }
        if n > MENU_ROWS {
            queue!(
                out,
                Print("\r\n"),
                SetForegroundColor(Color::DarkGrey),
                Print(format!("  {}/{n}", sel + 1)),
                ResetColor
            )?;
            rows += 1;
        }
    }
    if editor.ctrl_c_armed() {
        queue!(
            out,
            Print("\r\n"),
            SetForegroundColor(Color::DarkGrey),
            Print("Press Ctrl+C again to quit"),
            ResetColor
        )?;
        rows += 1;
    }
    if rows > 0 {
        queue!(out, MoveUp(rows))?;
    }
    queue!(out, MoveToColumn((PROMPT.len() + cursor - start) as u16))?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(short_time("2026-10-19T21:04:11Z"), "2026-10-19 21:04");
        assert_eq!(short_map("/Game/Maps/Ascent/Ascent"), "Ascent");
        assert_eq!(short_map("Lotus"), "Lotus");
    }

    #[test]
    fn words() {
        assert_eq!(split_words("/packet  --match ab"), ["/packet", "--match", "ab"]);
        assert_eq!(
            split_words(r#"/init --vrfkit "C:\Program Files\vrfkit.exe""#),
            ["/init", "--vrfkit", r"C:\Program Files\vrfkit.exe"]
        );
        assert_eq!(
            split_words(r"/scan --demos-dir C:\x"),
            ["/scan", "--demos-dir", r"C:\x"]
        );
        assert!(split_words("   ").is_empty());
    }

    #[test]
    fn every_command_is_listed_with_its_flags() {
        let cmds = commands();
        let names: Vec<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
        for n in [
            "scan", "packet", "matches", "trends", "status", "sync", "init", "players", "help", "exit",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        assert!(cmds.iter().all(|c| !c.about.is_empty()));
        let packet = cmds.iter().find(|c| c.name == "packet").unwrap();
        let m = packet.flags.iter().find(|f| f.long == "--match").unwrap();
        assert!(m.value.is_some() && !m.help.is_empty());
        let scan = cmds.iter().find(|c| c.name == "scan").unwrap();
        assert!(scan.flags.iter().any(|f| f.long == "--keep-raw" && f.value.is_none()));
        assert!(scan.flags.iter().all(|f| f.long != "--data-dir"));
    }
}
