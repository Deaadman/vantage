//! `valreview` with no command: a full-screen shell. Output fills the top of
//! the terminal; the input box stays pinned to the bottom, with the command
//! menu opening above it. Commands run on a worker thread and print into the
//! transcript through `out`, so the screen keeps drawing while they work.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{CommandFactory, Parser};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, SetTitle};
use crossterm::{execute, queue};
use valreview_core::Catalog;
use valreview_core::builds::check_game_version;
use valreview_core::config::Config;
use valreview_core::install::{self, Layout};
use valreview_core::paths;
use valreview_core::store::Store;
use valreview_core::update;
use valreview_core::vrfkit::Vrfkit;

use crate::Cli;
use crate::editor::{Action, CommandInfo, Context, Editor, FlagInfo, MatchInfo};
use crate::out::{self, Kind};

const PROMPT: &str = "❯ ";
const MENU_ROWS: usize = 8;
const HISTORY_LINES: usize = 500;
const RECENT_MATCHES: usize = 5;
const TICK: Duration = Duration::from_millis(80);
const SPINNER: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];

/// Valorant red: the logo and problems.
const RED: Color = Color::Rgb { r: 255, g: 70, b: 85 };
/// The highlighted menu row and command names.
const ACCENT: Color = Color::Rgb { r: 177, g: 185, b: 249 };
const YELLOW: Color = Color::Rgb { r: 255, g: 193, b: 7 };
const GREY: Color = Color::Rgb { r: 140, g: 140, b: 140 };
const RULE: Color = Color::Rgb { r: 110, g: 110, b: 110 };
const BRIGHT: Color = Color::Rgb { r: 240, g: 240, b: 240 };
const ECHO_BG: Color = Color::Rgb { r: 45, g: 45, b: 45 };

/// The terminal tab title while the shell runs.
const TITLE: &str = "Val Review";

/// A pixel crosshair, three rows of half blocks.
const LOGO: [&str; 3] = ["▄▀▀▀█▀▀▀▄", "█▄▄ ▀ ▄▄█", "▀▄▄▄█▄▄▄▀"];

/// Shell-only commands, after the clap ones.
const BUILTINS: [(&str, &str); 3] = [
    ("help", "Show the commands and keys"),
    ("clear", "Clear the screen"),
    ("exit", "Quit valreview"),
];

pub fn run(data_dir: &Path) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    out::capture(Some(tx));
    let result = Screen::on().map_err(anyhow::Error::from).and_then(|_screen| {
        let mut app = App::new(data_dir, rx);
        let (update_tx, update_rx) = mpsc::channel();
        app.update_rx = Some(update_rx);
        let cfg = Config::load(data_dir).unwrap_or_default();
        if update::auto_disabled(cfg.auto_update).is_none() {
            let dir = data_dir.to_path_buf();
            std::thread::spawn(move || check_for_update(&dir, &update_tx));
        }
        let dir = data_dir.to_path_buf();
        app.start("Scanning replays", move || {
            launch(&dir);
            Flow::Continue
        });
        app.event_loop()
    });
    out::capture(None);
    result
}

/// The alternate screen in raw mode, titled; restored however the shell ends.
struct Screen {
    /// The title to put back.
    old_title: Option<String>,
}

impl Screen {
    fn on() -> io::Result<Self> {
        let old_title = console_title();
        execute!(io::stdout(), EnterAlternateScreen, SetTitle(TITLE))?;
        let screen = Screen { old_title };
        terminal::enable_raw_mode()?;
        let _ = execute!(io::stdout(), event::EnableBracketedPaste);
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), event::DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        if let Some(t) = &self.old_title {
            let _ = execute!(io::stdout(), SetTitle(t));
        }
    }
}

/// The console window's title now, so the shell can restore it.
#[cfg(windows)]
fn console_title() -> Option<String> {
    let mut buf = [0u16; 1024];
    // SAFETY: the buffer outlives the call and its length is passed with it.
    let n = unsafe { windows_sys::Win32::System::Console::GetConsoleTitleW(buf.as_mut_ptr(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())]))
}

#[cfg(not(windows))]
fn console_title() -> Option<String> {
    None
}

enum Flow {
    Continue,
    Clear,
    Exit,
}

enum Item {
    /// The logo, with two lines of setup beside the name.
    Header {
        setup: String,
        folder: String,
    },
    Line(Kind, String),
    /// A line the user sent.
    Echo(String),
}

struct Job {
    label: String,
    started: Instant,
    handle: JoinHandle<Flow>,
}

struct App {
    data_dir: PathBuf,
    ctx: Context,
    editor: Editor,
    history_path: PathBuf,
    transcript: Vec<Item>,
    /// Rows scrolled up from the newest output.
    scroll: usize,
    rx: Receiver<(Kind, String)>,
    job: Option<Job>,
    notices: Vec<(Color, String)>,
    /// What the background update check found, shown before the notices.
    update_note: Option<(Color, String)>,
    update_rx: Option<Receiver<(Color, String)>>,
    footer: String,
    tick: usize,
}

impl App {
    fn new(data_dir: &Path, rx: Receiver<(Kind, String)>) -> Self {
        let history_path = data_dir.join("history.txt");
        let mut app = App {
            data_dir: data_dir.to_path_buf(),
            ctx: Context {
                commands: commands(),
                matches: vec![],
            },
            editor: Editor::new(load_history(&history_path)),
            history_path,
            transcript: vec![],
            scroll: 0,
            rx,
            job: None,
            notices: vec![],
            update_note: None,
            update_rx: None,
            footer: String::new(),
            tick: 0,
        };
        app.clear();
        app.refresh();
        app
    }

    /// Empty the transcript down to the header.
    fn clear(&mut self) {
        let (setup, folder) = header_text(&self.data_dir);
        self.transcript = vec![Item::Header { setup, folder }];
        self.scroll = 0;
    }

    /// Re-read what commands may have changed: matches, `me`, setup notices.
    fn refresh(&mut self) {
        self.ctx.matches = matches(&self.data_dir);
        self.notices = notices(&self.data_dir);
        let me = Config::load(&self.data_dir)
            .ok()
            .and_then(|c| c.me)
            .map_or_else(|| "me not set".to_owned(), |m| format!("me {}", &m[..m.len().min(8)]));
        let n = self.ctx.matches.len();
        self.footer = format!("{me} · {n} match{} · /help for keys", if n == 1 { "" } else { "es" });
        if let Some(Item::Header { setup, folder }) = self.transcript.first_mut() {
            (*setup, *folder) = header_text(&self.data_dir);
        }
    }

    fn start(&mut self, label: &str, job: impl FnOnce() -> Flow + Send + 'static) {
        self.job = Some(Job {
            label: label.to_owned(),
            started: Instant::now(),
            handle: std::thread::spawn(job),
        });
    }

    fn drain(&mut self) -> bool {
        let mut any = false;
        while let Ok((kind, text)) = self.rx.try_recv() {
            self.transcript.push(Item::Line(kind, text));
            self.scroll = 0;
            any = true;
        }
        any
    }

    fn event_loop(&mut self) -> Result<()> {
        let mut out = io::stdout();
        let mut dirty = true;
        loop {
            dirty |= self.drain();
            if let Some(note) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.update_note = Some(note);
                dirty = true;
            }
            if self.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
                let job = self.job.take().expect("checked");
                let flow = job.handle.join();
                self.drain();
                match flow {
                    Ok(Flow::Exit) => return Ok(()),
                    Ok(Flow::Clear) => self.clear(),
                    Ok(Flow::Continue) => {}
                    Err(_) => self.transcript.push(Item::Line(
                        Kind::Error,
                        "internal error: the command crashed (please report)".into(),
                    )),
                }
                self.refresh();
                dirty = true;
            }
            if self.job.is_some() {
                self.tick += 1;
                dirty = true;
            }
            if dirty {
                self.draw(&mut out)?;
                dirty = false;
            }
            if !event::poll(TICK)? {
                continue;
            }
            dirty = true;
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if !self.key(k) {
                        return Ok(());
                    }
                }
                Event::Paste(s) => self.editor.insert_str(&s),
                _ => {}
            }
        }
    }

    /// Handle one key. False to quit.
    fn key(&mut self, key: KeyEvent) -> bool {
        let page = terminal::size().map_or(10, |(_, h)| (h as usize / 2).max(1));
        match key.code {
            KeyCode::PageUp => {
                self.scroll += page;
                return true;
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(page);
                return true;
            }
            // One command at a time; the typed line waits.
            KeyCode::Enter if self.job.is_some() => return true,
            _ => {}
        }
        match self.editor.handle(key, &self.ctx) {
            Action::None => {}
            Action::Exit => return false,
            Action::ClearScreen => self.clear(),
            Action::Submit(line) if line.is_empty() => {}
            Action::Submit(line) => {
                save_history(&self.history_path, &line);
                self.transcript.push(Item::Echo(line.clone()));
                self.scroll = 0;
                let label = format!("Running {}", line.split_whitespace().next().unwrap_or(&line));
                let dir = self.data_dir.clone();
                self.start(&label, move || execute_line(&line, &dir));
            }
        }
        true
    }

    /// Redraw the whole screen: transcript on top, then (bottom-up) the
    /// footer, a rule, the input, a rule, and the menu or the status line.
    fn draw(&mut self, out: &mut impl Write) -> io::Result<()> {
        let (w, h) = terminal::size().map(|(w, h)| (w as usize, h as usize))?;
        let (w, h) = (w.max(20), h.max(8));

        let s = self.editor.suggestions(&self.ctx);
        let above = if s.items.is_empty() {
            vec![self.status_row(w)]
        } else {
            let typed: String = self
                .editor
                .line()
                .chars()
                .take(self.editor.cursor())
                .skip(s.start)
                .collect();
            menu_rows(&s.items, self.editor.selected(s.items.len()), &typed)
        };
        let (input, cursor_col) = self.input_row(w);
        let mut bottom = above;
        bottom.push(rule(w));
        bottom.push(input);
        bottom.push(rule(w));
        let footer = if self.editor.ctrl_c_armed() {
            "Press Ctrl+C again to quit"
        } else {
            self.footer.as_str()
        };
        bottom.push(vec![(Style::fg(GREY), format!("  {footer}"))]);

        let area = h.saturating_sub(bottom.len());
        let rows = transcript_rows(&self.transcript, w);
        let max_scroll = rows.len().saturating_sub(area);
        self.scroll = self.scroll.min(max_scroll);
        let end = rows.len() - self.scroll;
        let visible = &rows[end.saturating_sub(area)..end];

        queue!(out, Hide)?;
        for y in 0..area {
            queue!(out, MoveTo(0, y as u16))?;
            if let Some(row) = visible.get(y) {
                print_row(out, row, w)?;
            }
            queue!(out, Clear(ClearType::UntilNewLine))?;
        }
        for (i, row) in bottom.iter().enumerate() {
            queue!(out, MoveTo(0, (area + i) as u16))?;
            print_row(out, row, w)?;
            queue!(out, Clear(ClearType::UntilNewLine))?;
        }
        let input_y = h - 3;
        queue!(out, MoveTo(cursor_col as u16, input_y as u16), Show)?;
        out.flush()
    }

    /// Above the box: what is running or how far back you scrolled, and
    /// setup notices on the right.
    fn status_row(&self, w: usize) -> Row {
        let mut row = Row::new();
        if let Some(job) = &self.job {
            let secs = job.started.elapsed().as_secs();
            row.push((Style::fg(RED), format!("  {} ", SPINNER[self.tick / 2 % SPINNER.len()])));
            row.push((Style::fg(ACCENT), format!("{}… ", job.label)));
            row.push((Style::fg(GREY), format!("({secs}s)")));
        } else if self.scroll > 0 {
            row.push((Style::fg(GREY), format!("  ↓ {} more lines · PgDn", self.scroll)));
        }
        let mut right = Row::new();
        for (i, (color, text)) in self.update_note.iter().chain(&self.notices).enumerate() {
            if i > 0 {
                right.push((Style::fg(GREY), " · ".into()));
            }
            right.push((Style::fg(*color), text.clone()));
        }
        let (lw, rw) = (row_width(&row), row_width(&right));
        if lw + rw + 3 <= w {
            row.push((Style::default(), " ".repeat(w - lw - rw - 2)));
            row.extend(right);
        }
        row
    }

    /// The input line, scrolled sideways to keep the cursor visible, and the cursor column.
    fn input_row(&self, w: usize) -> (Row, usize) {
        let prompt_w = PROMPT.chars().count();
        let room = w.saturating_sub(prompt_w + 1);
        let line: Vec<char> = self.editor.line().chars().collect();
        let cursor = self.editor.cursor();
        let start = cursor.saturating_sub(room);
        let shown: String = line[start..line.len().min(start + room)].iter().collect();
        let mut row: Row = vec![(Style::fg(GREY), PROMPT.into())];
        let name_end = shown.find(char::is_whitespace).unwrap_or(shown.len());
        let name = shown[..name_end].trim_start_matches('/');
        if start == 0 && shown.starts_with('/') && self.ctx.commands.iter().any(|c| c.name == name) {
            row.push((Style::fg(ACCENT), shown[..name_end].to_owned()));
            row.push((Style::default(), shown[name_end..].to_owned()));
        } else {
            row.push((Style::default(), shown.clone()));
        }
        let ghost = if line.is_empty() {
            Some(if self.ctx.matches.is_empty() {
                "Try \"/status\"".to_owned()
            } else {
                "Try \"/packet\" to review your newest match".to_owned()
            })
        } else {
            self.editor.ghost(&self.ctx)
        };
        if let Some(g) = ghost {
            row.push((Style::fg(GREY), g));
        }
        (row, prompt_w + cursor - start)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
}

impl Style {
    fn fg(c: Color) -> Self {
        Style {
            fg: Some(c),
            ..Style::default()
        }
    }

    fn bold(self) -> Self {
        Style { bold: true, ..self }
    }
}

/// One screen row as styled pieces.
type Row = Vec<(Style, String)>;

fn row_width(row: &Row) -> usize {
    row.iter().map(|(_, s)| s.chars().count()).sum()
}

/// Print a row clipped to `w` columns (one short, so the last column never wraps).
fn print_row(out: &mut impl Write, row: &Row, w: usize) -> io::Result<()> {
    let mut left = w.saturating_sub(1);
    for (style, text) in row {
        if left == 0 {
            break;
        }
        let piece: String = text.chars().take(left).collect();
        left -= piece.chars().count();
        if let Some(c) = style.fg {
            queue!(out, SetForegroundColor(c))?;
        }
        if let Some(c) = style.bg {
            queue!(out, SetBackgroundColor(c))?;
        }
        if style.bold {
            queue!(out, SetAttribute(Attribute::Bold))?;
        }
        queue!(out, Print(piece), SetAttribute(Attribute::Reset), ResetColor)?;
    }
    Ok(())
}

fn rule(w: usize) -> Row {
    vec![(Style::fg(RULE), "─".repeat(w.saturating_sub(1)))]
}

/// The transcript as screen rows, long lines wrapped, one column of margin.
fn transcript_rows(items: &[Item], w: usize) -> Vec<Row> {
    let text_w = w.saturating_sub(3).max(10);
    let mut rows = Vec::new();
    for item in items {
        match item {
            Item::Header { setup, folder } => {
                rows.push(vec![]);
                let text: [Row; 3] = [
                    vec![
                        (Style::default().bold(), "valreview".into()),
                        (Style::fg(GREY), format!(" v{}", env!("CARGO_PKG_VERSION"))),
                    ],
                    vec![(Style::fg(GREY), setup.clone())],
                    vec![(Style::fg(GREY), folder.clone())],
                ];
                for (logo, text) in LOGO.iter().zip(text) {
                    let mut row = vec![(Style::fg(RED), format!(" {logo}")), (Style::default(), "   ".into())];
                    row.extend(text);
                    rows.push(row);
                }
                rows.push(vec![]);
            }
            Item::Line(kind, text) => {
                let style = match kind {
                    Kind::Out => Style::default(),
                    Kind::Warn => Style::fg(YELLOW),
                    Kind::Error => Style::fg(RED),
                    Kind::Dim => Style::fg(GREY),
                    Kind::Bold => Style::default().bold(),
                };
                for piece in wrap(text, text_w) {
                    rows.push(vec![(style, format!(" {piece}"))]);
                }
            }
            Item::Echo(text) => {
                rows.push(vec![]);
                let bg = Style {
                    bg: Some(ECHO_BG),
                    ..Style::default()
                };
                for (i, piece) in wrap(text, text_w).into_iter().enumerate() {
                    let lead = if i == 0 { PROMPT } else { "  " };
                    let pad = text_w.saturating_sub(piece.chars().count());
                    rows.push(vec![
                        (Style { fg: Some(GREY), ..bg }, format!(" {lead}")),
                        (Style { fg: Some(BRIGHT), ..bg }, format!("{piece}{}", " ".repeat(pad))),
                    ]);
                }
                rows.push(vec![]);
            }
        }
    }
    rows
}

/// Wrap at spaces into rows of at most `w` chars, breaking a longer word
/// where it must. Continuation rows keep the line's indent.
fn wrap(text: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let chars: Vec<char> = text.chars().collect();
    let indent = chars.iter().take_while(|c| **c == ' ').count().min(w / 2);
    let mut rows = Vec::new();
    let mut rest = &chars[..];
    let mut first = true;
    loop {
        let lead = if first { 0 } else { indent };
        let room = w - lead;
        if rest.len() <= room {
            rows.push(" ".repeat(lead) + &rest.iter().collect::<String>());
            return rows;
        }
        // The last space that leaves something on this row, else a hard break.
        let cut = rest[..=room]
            .iter()
            .rposition(|c| *c == ' ')
            .filter(|&i| rest[..i].iter().any(|c| *c != ' '))
            .unwrap_or(room);
        rows.push(" ".repeat(lead) + rest[..cut].iter().collect::<String>().trim_end());
        rest = &rest[cut..];
        while rest.first() == Some(&' ') {
            rest = &rest[1..];
        }
        first = false;
        if rest.is_empty() {
            return rows;
        }
    }
}

/// The menu, top to bottom, ending just above the input box.
fn menu_rows(items: &[crate::editor::Suggestion], sel: usize, typed: &str) -> Vec<Row> {
    let query = typed.trim_start_matches('/').to_lowercase();
    let n = items.len();
    let first = sel.saturating_sub(MENU_ROWS - 1).min(n.saturating_sub(MENU_ROWS));
    let label_w = items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(16, 32);
    let mut rows = Vec::new();
    for (i, item) in items.iter().enumerate().skip(first).take(MENU_ROWS) {
        let label = format!("{:<label_w$}", item.label);
        let mut row: Row = Vec::new();
        if i == sel {
            row.push((Style::fg(ACCENT), "  ❯ ".into()));
            row.push((Style::fg(ACCENT), label));
            row.push((Style::fg(ACCENT), format!("  {}", item.description)));
        } else {
            row.push((Style::default(), "    ".into()));
            row.extend(highlight(&label, &query, false, Style::fg(GREY)));
            row.push((Style::fg(GREY), "  ".into()));
            row.extend(highlight(&item.description, &query, true, Style::fg(GREY)));
        }
        rows.push(row);
    }
    if n > MENU_ROWS {
        rows.push(vec![(Style::fg(GREY), format!("    {}/{n} · ↑↓ for more", sel + 1))]);
    }
    rows
}

/// `text` split into plain and bold-white pieces where it matches `query`:
/// the first occurrence, or (`words`) every word that starts with it.
fn highlight(text: &str, query: &str, words: bool, base: Style) -> Row {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let q: Vec<char> = query.chars().collect();
    let mut marked = vec![false; chars.len()];
    if !q.is_empty() && lower.len() == chars.len() {
        let matches_at = |i: usize| lower.get(i..i + q.len()) == Some(&q[..]);
        if words {
            for i in 0..chars.len() {
                let word_start = i == 0 || !chars[i - 1].is_alphanumeric();
                if word_start && matches_at(i) {
                    marked[i..i + q.len()].fill(true);
                }
            }
        } else if let Some(i) = (0..chars.len()).find(|&i| matches_at(i)) {
            marked[i..i + q.len()].fill(true);
        }
    }
    let mut row = Row::new();
    for (c, m) in chars.into_iter().zip(marked) {
        let style = if m { Style::fg(BRIGHT).bold() } else { base };
        match row.last_mut() {
            Some((s, text)) if *s == style => text.push(c),
            _ => row.push((style, c.to_string())),
        }
    }
    row
}

/// Beside the logo: vrfkit and game build, then the replay folder.
fn header_text(data_dir: &Path) -> (String, String) {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let vrfkit = Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref())
        .map_or_else(|_| "vrfkit not set up".to_owned(), |v| format!("vrfkit {}", v.version));
    let game = Catalog::load(&paths::catalog_path(data_dir))
        .ok()
        .and_then(|c| c.game_version)
        .map_or_else(
            || "no catalog yet".to_owned(),
            |v| format!("game {}", v.splitn(3, '.').take(2).collect::<Vec<_>>().join(".")),
        );
    let folder = cfg
        .demos_dir()
        .map_or_else(|| "no replay folder set".to_owned(), |d| d.display().to_string());
    (format!("{vrfkit} · {game}"), folder)
}

/// Setup problems and updates, for the right of the status line.
fn notices(data_dir: &Path) -> Vec<(Color, String)> {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let mut out = Vec::new();
    if Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()).is_err() {
        out.push((RED, "vrfkit not installed · Run /update".to_owned()));
    }
    if !cfg.demos_dir().is_some_and(|d| d.is_dir()) {
        out.push((YELLOW, "No replays yet · download a match in the client".to_owned()));
    }
    match Catalog::load(&paths::catalog_path(data_dir)) {
        Err(_) => out.push((YELLOW, "No catalog · Run /sync".to_owned())),
        Ok(c) => {
            if c.game_version.as_deref().and_then(check_game_version).is_some() {
                out.push((YELLOW, "Game patched past what vrfkit reads · update vrfkit".to_owned()));
            } else if let Some(days) = c.age_days(valreview_core::time::now_unix()).filter(|&d| d > 30) {
                out.push((YELLOW, format!("Catalog {days} days old · Run /sync")));
            }
        }
    }
    out
}

/// On launch: fetch the catalog if there is none, scan, set `me`, list recent matches.
fn launch(data_dir: &Path) {
    let cfg = Config::load(data_dir).unwrap_or_default();
    if !paths::catalog_path(data_dir).is_file() {
        dim("Fetching agent, map and weapon names from valorant-api.com (first run only)");
        if let Err(e) = crate::run_command(data_dir, crate::Command::Sync) {
            errln!("sync failed, using built-in names: {e:#}");
        }
        outln!();
    }
    if let Err(e) = Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()) {
        outln!("Not set up yet: {e:#}");
        dim("Run /update to install it (or /init --vrfkit <path> for your own copy), then /scan.");
        return;
    }
    match cfg.demos_dir() {
        Some(d) if d.is_dir() => {
            if let Err(e) = crate::scan_cmd(data_dir, None, false, false, false) {
                out::emit(Kind::Error, format!("scan failed: {e:#}"));
            }
            if cfg.me.is_none() {
                detect_me(data_dir);
            }
        }
        Some(d) => {
            outln!("No replays yet: {} does not exist.", d.display());
            dim("Download a match in the client's career tab, then run /scan.");
        }
        None => dim("No replay folder set: /init --demos-dir <folder>"),
    }
    recent(&matches(data_dir));
}

/// In the background on launch: install a newer release (it applies on the
/// next launch), or only say it exists when this exe was not installed by
/// the installer (a dev build). Offline or failing checks stay quiet.
fn check_for_update(data_dir: &Path, tx: &Sender<(Color, String)>) {
    let installed = Layout::new()
        .ok()
        .filter(|l| std::env::current_exe().is_ok_and(|e| l.contains(&e)));
    if let Some(layout) = &installed {
        install::cleanup(layout, crate::VERSION);
    }
    let manifest = match update::latest() {
        Ok(m) => m,
        Err(e) => {
            update::State::record(data_dir, format!("check failed: {e:#}"));
            return;
        }
    };
    let v = manifest.version.clone();
    if !install::is_newer(&v, crate::VERSION) {
        update::State::record(data_dir, format!("up to date ({})", crate::VERSION));
        return;
    }
    let Some(layout) = installed else {
        update::State::record(data_dir, format!("{v} available"));
        let _ = tx.send((YELLOW, format!("Update {v} available · Run /update")));
        return;
    };
    let _ = tx.send((GREY, format!("Updating to {v}…")));
    match update::apply(&layout, &manifest, data_dir) {
        Ok(_) => {
            update::State::record(data_dir, format!("installed {v}"));
            let _ = tx.send((ACCENT, format!("Updated to {v} · Restart to apply")));
        }
        Err(e) => {
            update::State::record(data_dir, format!("failed: {e:#}"));
            let _ = tx.send((YELLOW, "Update failed · Run /update".to_owned()));
        }
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
        outln!(
            "You are {} (the only account in all {n} matches); change it with /init --me",
            subjects[0]
        );
    }
}

fn recent(matches: &[MatchInfo]) {
    if matches.is_empty() {
        return;
    }
    outln!();
    out::emit(Kind::Bold, "Recent matches".into());
    for m in matches.iter().take(RECENT_MATCHES) {
        outln!("  {}  {}", &m.id[..m.id.len().min(8)], m.label);
    }
    if matches.len() > RECENT_MATCHES {
        dim(&format!("  …and {} more (/matches)", matches.len() - RECENT_MATCHES));
    }
    dim("/packet reviews the newest; /packet <space> picks another");
}

fn dim(s: &str) {
    out::emit(Kind::Dim, s.to_owned());
}

/// The clap subcommands and their long flags, then the built-ins.
fn commands() -> Vec<CommandInfo> {
    let mut out: Vec<CommandInfo> = Cli::command()
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
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

/// Run one line (on the worker thread).
fn execute_line(line: &str, data_dir: &Path) -> Flow {
    let mut words = split_words(line);
    let Some(first) = words.first_mut() else {
        return Flow::Continue;
    };
    *first = first.trim_start_matches('/').to_lowercase();
    match first.as_str() {
        "exit" | "quit" => return Flow::Exit,
        "clear" => return Flow::Clear,
        "help" => {
            help();
            return Flow::Continue;
        }
        _ => {}
    }
    let cli = match Cli::try_parse_from(std::iter::once("valreview".to_owned()).chain(words)) {
        Ok(cli) => cli,
        Err(e) => {
            // Also how `--help` prints.
            let kind = if e.use_stderr() { Kind::Error } else { Kind::Out };
            out::emit(kind, e.render().to_string().trim_end().to_owned());
            return Flow::Continue;
        }
    };
    let Some(command) = cli.command else {
        return Flow::Continue;
    };
    let dir = cli.data_dir.unwrap_or_else(|| data_dir.to_path_buf());
    if let Err(e) = crate::run_command(&dir, command) {
        out::emit(Kind::Error, format!("error: {e:#}"));
    }
    Flow::Continue
}

fn help() {
    for c in commands() {
        outln!("  {:<10} {}", format!("/{}", c.name), c.about);
    }
    outln!();
    dim("Keys: Tab or → completes, ↑↓ move in the menu (or through history), Enter runs,");
    dim("Esc hides the menu, PgUp/PgDn scroll the output, Ctrl+U clears to the start,");
    dim("Ctrl+W deletes a word, Ctrl+L clears the screen, Ctrl+C twice quits.");
    dim("Every command takes --help, e.g. /packet --help.");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(row: &Row) -> String {
        row.iter().map(|(_, s)| s.as_str()).collect()
    }

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

    #[test]
    fn wrapping() {
        assert_eq!(wrap("", 4), [""]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("3 replays: 0 new, 3 done", 12), ["3 replays: 0", "new, 3 done"]);
        assert_eq!(wrap("  ab cd ef", 6), ["  ab", "  cd", "  ef"]);
        assert_eq!(wrap("  abcdefgh", 6), ["  abcd", "  efgh"]);
        let rows = transcript_rows(&[Item::Line(Kind::Out, "abcdefghij klmn".into())], 13);
        assert_eq!(rows.iter().map(text).collect::<Vec<_>>(), [" abcdefghij", " klmn"]);
        let rows = transcript_rows(
            &[Item::Header {
                setup: "vrfkit v0.3.0 · game 13.08".into(),
                folder: "C:\\Demos".into(),
            }],
            80,
        );
        assert_eq!(rows.len(), 5);
        assert!(text(&rows[1]).contains("valreview v"));
    }

    #[test]
    fn highlighting() {
        let base = Style::fg(GREY);
        let bold = Style::fg(BRIGHT).bold();
        assert_eq!(
            highlight("/model", "m", false, base),
            vec![
                (base, "/".to_owned()),
                (bold, "m".to_owned()),
                (base, "odel".to_owned())
            ]
        );
        // Descriptions: every word that starts with the query.
        let row = highlight("Set the model, mostly", "m", true, base);
        let marked: Vec<&str> = row
            .iter()
            .filter(|(s, _)| *s == bold)
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(marked, ["m", "m"]);
        assert_eq!(highlight("abc", "", true, base), vec![(base, "abc".to_owned())]);
    }

    #[test]
    fn menu_marks_the_selection() {
        let item = |v: &str| crate::editor::Suggestion {
            value: v.into(),
            label: v.into(),
            description: format!("about {v}"),
        };
        let items: Vec<_> = (0..10).map(|i| item(&format!("/c{i}"))).collect();
        let rows = menu_rows(&items, 9, "/c");
        assert_eq!(rows.len(), MENU_ROWS + 1, "a window of rows plus the count");
        assert!(text(&rows[MENU_ROWS - 1]).starts_with("  ❯ /c9"));
        assert!(text(&rows[MENU_ROWS]).contains("10/10"));
    }
}
