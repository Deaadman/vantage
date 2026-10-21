//! `vantage` with no command: a full-screen shell. Output fills the top of
//! the terminal; the input box stays pinned to the bottom, with the command
//! menu opening above it. Commands run on a worker thread and print into the
//! transcript through `out`, so the screen keeps drawing while they work.

use std::collections::HashMap;
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
use vantage_core::Catalog;
use vantage_core::ai;
use vantage_core::builds::check_game_version;
use vantage_core::config::Config;
use vantage_core::install::{self, Layout};
use vantage_core::paths;
use vantage_core::store::Store;
use vantage_core::update::{self, WhatsNew};
use vantage_core::vrfkit::Vrfkit;

use crate::coach_views::{CoachAction, HabitsPanel, PlanData, PlanPanel, RankPanel};
use crate::editor::{Action, CommandInfo, Context, Editor, FlagInfo, MatchInfo};
use crate::help::{HelpAction, HelpPanel};
use crate::logo;
use crate::out::{self, Kind};
use crate::panel::rank_color;
use crate::review_view::{ReviewAction, ReviewPanel};
use crate::settings::{SettingsAction, SettingsPanel};
use crate::{APP_NAME, Cli, ReviewData};

const PROMPT: &str = "❯ ";
const MENU_ROWS: usize = 8;
const HISTORY_LINES: usize = 500;
const TICK: Duration = Duration::from_millis(80);
/// How often the replay folder is checked for new downloads.
const WATCH_EVERY: Duration = Duration::from_secs(3);
/// A catalog this many days old is refreshed in the background on launch.
const SYNC_AFTER_DAYS: i64 = 1;

/// The working spinner: a diamond that beeps like the planted spike, once
/// every 1.2 s at first and quicker the longer the job runs, down to 0.24 s.
pub(crate) fn spike(elapsed: Duration) -> (Style, &'static str) {
    let tick = elapsed.as_millis() / TICK.as_millis();
    let gap = 15u128.saturating_sub(elapsed.as_secs() as u128 / 4).max(3);
    match tick % gap {
        0 => (Style::fg(BRIGHT), "◆"),
        1 => (Style::fg(RED), "◆"),
        _ => (Style::fg(DIM_RED), "◇"),
    }
}

/// Valorant red: the logo and problems.
pub(crate) const RED: Color = Color::Rgb { r: 255, g: 70, b: 85 };
/// The spike between beeps.
const DIM_RED: Color = Color::Rgb { r: 137, g: 40, b: 48 };
/// The highlighted menu row and command names.
pub(crate) const ACCENT: Color = Color::Rgb { r: 177, g: 185, b: 249 };
pub(crate) const YELLOW: Color = Color::Rgb { r: 255, g: 193, b: 7 };
pub(crate) const GREY: Color = Color::Rgb { r: 140, g: 140, b: 140 };
const RULE: Color = Color::Rgb { r: 110, g: 110, b: 110 };
pub(crate) const BRIGHT: Color = Color::Rgb { r: 240, g: 240, b: 240 };
const ECHO_BG: Color = Color::Rgb { r: 45, g: 45, b: 45 };

/// Shell-only commands, after the clap ones.
const BUILTINS: [(&str, &str); 3] = [
    ("help", "Show the commands and keys"),
    ("clear", "Clear the screen"),
    ("exit", "Quit Vantage"),
];

/// What the background update check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateNote {
    /// Not installed: automatic updates are off, or this is a dev build.
    Available,
    /// Installed; it runs from the next launch.
    Installed,
    Failed,
}

impl UpdateNote {
    fn text(self) -> (Color, &'static str) {
        match self {
            UpdateNote::Available => (YELLOW, "Update available! /update to install it"),
            UpdateNote::Installed => (ACCENT, "Update available! Restart to apply"),
            UpdateNote::Failed => (RED, "Update failed · /update to try again"),
        }
    }
}

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
            std::thread::spawn(move || check_for_update(&dir, &update_tx, true));
        } else {
            let dir = data_dir.to_path_buf();
            std::thread::spawn(move || check_for_update(&dir, &update_tx, false));
        }
        // With no catalog yet, `launch` fetches one first; an old one refreshes quietly.
        let path = paths::catalog_path(data_dir);
        let stale = Catalog::load(&path).ok().is_some_and(|c| {
            c.age_days(vantage_core::time::now_unix())
                .is_none_or(|d| d >= SYNC_AFTER_DAYS)
        });
        if stale {
            std::thread::spawn(move || {
                if let Ok(cat) = vantage_core::catalog::fetch() {
                    let _ = cat.save(&path);
                }
            });
        }
        if paths::catalog_path(data_dir).is_file() {
            app.start_launch();
        } else {
            // First run: names for agents, maps and weapons before anything else.
            let path = paths::catalog_path(data_dir);
            app.start("Fetching VALORANT data", false, move || {
                if let Err(e) = fetch_catalog(&path) {
                    errln!("Couldn't fetch VALORANT data, so built-in names are used: {e:#}");
                }
                Flow::Launch
            });
        }
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
        execute!(io::stdout(), EnterAlternateScreen, SetTitle(APP_NAME))?;
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
    /// Start the launch scan (after the first-run fetch).
    Launch,
    Clear,
    Exit,
    /// Open a panel.
    Review(Box<ReviewData>),
    Plan(Box<PlanData>),
    Habits(Box<HabitsPanel>),
    Rank(Box<RankPanel>),
    /// Open the settings panel.
    Settings,
    /// The AI review finished (for the open review).
    AiDone(Result<ai::Saved, String>),
}

/// What covers the whole screen.
enum Panel {
    Review(ReviewPanel),
    Plan(PlanPanel),
    Habits(Box<HabitsPanel>),
    Rank(Box<RankPanel>),
}

/// What a panel's key asks the shell to do.
enum PanelAction {
    None,
    Close,
    Review(ReviewAction),
    Focus(&'static str),
}

impl Panel {
    fn key(&mut self, key: KeyEvent) -> PanelAction {
        let coach = |a: CoachAction| match a {
            CoachAction::None => PanelAction::None,
            CoachAction::Close => PanelAction::Close,
            CoachAction::Focus(s) => PanelAction::Focus(s),
        };
        match self {
            Panel::Review(p) => match p.key(key) {
                ReviewAction::None => PanelAction::None,
                ReviewAction::Close => PanelAction::Close,
                a => PanelAction::Review(a),
            },
            Panel::Plan(p) => coach(p.key(key)),
            Panel::Habits(p) => coach(p.key(key)),
            Panel::Rank(p) => coach(p.key(key)),
        }
    }

    fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        match self {
            Panel::Review(p) => p.rows(w, h),
            Panel::Plan(p) => p.rows(w, h),
            Panel::Habits(p) => p.rows(w, h),
            Panel::Rank(p) => p.rows(w, h),
        }
    }
}

enum Item {
    /// The logo, with the rank and game version, then the replay folder, beside the name.
    Header {
        rank: Option<String>,
        game: String,
        folder: String,
        /// The logo's frame: it plays once at launch, then stays on the last.
        logo: usize,
    },
    Line(Kind, String),
    /// A line of pieces in their own styles; never wrapped.
    Row(out::Line),
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
    rx: Receiver<out::Line>,
    job: Option<Job>,
    notices: Vec<(Color, String)>,
    /// What the background update check found, shown before the notices.
    update_note: Option<UpdateNote>,
    update_rx: Option<Receiver<UpdateNote>>,
    /// Whether a rank is set, for the input's placeholder.
    ranked: bool,
    footer: String,
    watch: Watch,
    /// The `/help` panel, drawn in place of the input while open.
    help: Option<HelpPanel>,
    /// The `/settings` panel, drawn in place of the input while open.
    settings: Option<SettingsPanel>,
    /// A full-screen panel while open.
    panel: Option<Panel>,
    /// When the shell opened, for the launch logo.
    launched: Instant,
}

/// The replay folder as of the last scan, and what has changed since.
struct Watch {
    last_poll: Instant,
    seen: HashMap<PathBuf, u64>,
    /// Changed files and their sizes at the previous poll.
    pending: HashMap<PathBuf, u64>,
}

/// Every replay in the configured folder with its size.
fn replay_sizes(data_dir: &Path) -> HashMap<PathBuf, u64> {
    let Some(dir) = Config::load(data_dir).ok().and_then(|c| c.demos_dir()) else {
        return HashMap::new();
    };
    vantage_core::scan::find_replays(&dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| Some((p.clone(), std::fs::metadata(&p).ok()?.len())))
        .collect()
}

fn is_help(line: &str) -> bool {
    line.trim().trim_start_matches('/').eq_ignore_ascii_case("help")
}

/// Runs `f` and turns its error into a transcript line.
fn panel_or_error<T>(f: impl FnOnce() -> Result<T>, open: impl FnOnce(T) -> Flow) -> Flow {
    match f() {
        Ok(v) => open(v),
        Err(e) => {
            out::emit(Kind::Error, format!("{e:#}"));
            Flow::Continue
        }
    }
}

impl App {
    fn new(data_dir: &Path, rx: Receiver<out::Line>) -> Self {
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
            ranked: false,
            footer: String::new(),
            watch: Watch {
                last_poll: Instant::now(),
                seen: HashMap::new(),
                pending: HashMap::new(),
            },
            help: None,
            settings: None,
            panel: None,
            launched: Instant::now(),
        };
        app.clear();
        app.refresh();
        app
    }

    /// Empty the transcript down to the header.
    fn clear(&mut self) {
        let (rank, game, folder) = header_text(&self.data_dir);
        let logo = self.logo_frame();
        self.transcript = vec![Item::Header {
            rank,
            game,
            folder,
            logo,
        }];
        self.scroll = 0;
    }

    /// The logo's frame by now: the last one once the launch animation is over.
    fn logo_frame(&self) -> usize {
        let f = self.launched.elapsed().as_millis() / TICK.as_millis();
        (f as usize).min(logo::FRAMES - 1)
    }

    /// Re-read what commands may have changed: matches, settings, notices.
    fn refresh(&mut self) {
        self.ctx.matches = matches(&self.data_dir);
        self.notices = notices(&self.data_dir, self.update_note.is_some());
        self.ranked = Config::load(&self.data_dir).is_ok_and(|c| c.rank.is_some());
        let n = self.ctx.matches.len();
        self.footer = format!(
            "{n} match{} · /help for commands and keys",
            if n == 1 { "" } else { "es" }
        );
        if let Some(Item::Header { rank, game, folder, .. }) = self.transcript.first_mut() {
            (*rank, *game, *folder) = header_text(&self.data_dir);
        }
    }

    /// Run `job` on a worker thread. A job that `scans` marks the replay
    /// folder's current files as seen, so the watcher only picks up later ones.
    fn start(&mut self, label: &str, scans: bool, job: impl FnOnce() -> Flow + Send + 'static) {
        if scans {
            self.watch.seen = replay_sizes(&self.data_dir);
            self.watch.pending.clear();
        }
        self.job = Some(Job {
            label: label.to_owned(),
            started: Instant::now(),
            handle: std::thread::spawn(job),
        });
    }

    /// The launch: what's new after an update, the scan, and the focus.
    fn start_launch(&mut self) {
        let dir = self.data_dir.clone();
        self.start("Checking for new replays", true, move || {
            launch(&dir);
            Flow::Continue
        });
    }

    /// Scan replays that appeared or changed since the last scan, once their
    /// size holds still for a poll (a download in progress keeps growing).
    fn poll_replays(&mut self) {
        if Config::load(&self.data_dir).is_ok_and(|c| c.watch == Some(false)) {
            self.watch.pending.clear();
            return;
        }
        let changed: HashMap<PathBuf, u64> = replay_sizes(&self.data_dir)
            .into_iter()
            .filter(|(p, size)| self.watch.seen.get(p) != Some(size))
            .collect();
        if changed.is_empty() || changed != self.watch.pending {
            self.watch.pending = changed;
            return;
        }
        let dir = self.data_dir.clone();
        self.start("Reading the new replay", true, move || {
            let stored = match crate::scan_cmd(&dir, None, false, false) {
                Ok(ids) => ids,
                Err(e) => {
                    out::emit(Kind::Error, format!("Couldn't read the new replay: {e:#}"));
                    vec![]
                }
            };
            if Config::load(&dir).is_ok_and(|c| c.me.is_none()) {
                detect_me(&dir);
            }
            // The newest stored match gets a line of its own.
            let newest = crate::my_matches(&dir).into_iter().find(|m| stored.contains(&m.id));
            if let Some(m) = newest {
                crate::new_match(&dir, &m.id);
            }
            Flow::Continue
        });
    }

    fn drain(&mut self) -> bool {
        let mut any = false;
        while let Ok(mut line) = self.rx.try_recv() {
            self.transcript.push(match line.len() {
                1 => {
                    let (kind, text) = line.remove(0);
                    Item::Line(kind, text)
                }
                _ => Item::Row(line),
            });
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
                self.notices = notices(&self.data_dir, true);
                dirty = true;
            }
            if self.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
                let job = self.job.take().expect("checked");
                let flow = job.handle.join();
                self.drain();
                if let Some(Panel::Review(p)) = &mut self.panel {
                    p.asking(None);
                }
                match flow {
                    Ok(Flow::Exit) => return Ok(()),
                    Ok(Flow::Clear) => self.clear(),
                    Ok(Flow::Continue) => {}
                    Ok(Flow::Launch) => self.start_launch(),
                    Ok(Flow::Review(data)) => self.panel = Some(Panel::Review(ReviewPanel::new(*data))),
                    Ok(Flow::Plan(data)) => self.panel = Some(Panel::Plan(PlanPanel::new(*data))),
                    Ok(Flow::Habits(p)) => self.panel = Some(Panel::Habits(p)),
                    Ok(Flow::Rank(p)) => self.panel = Some(Panel::Rank(p)),
                    Ok(Flow::Settings) => self.settings = Some(SettingsPanel::new(&self.data_dir)),
                    Ok(Flow::AiDone(result)) => {
                        if let Some(Panel::Review(p)) = &mut self.panel {
                            p.ai_done(result);
                        }
                    }
                    Err(_) => self.transcript.push(Item::Line(
                        Kind::Error,
                        format!(
                            "internal error: the command crashed (please report){}",
                            crate::session_log::hint()
                        ),
                    )),
                }
                self.refresh();
                dirty = true;
            }
            // A list the settings panel is fetching.
            if self.settings.as_mut().is_some_and(SettingsPanel::busy) {
                dirty = true;
            }
            if self.job.is_none() && self.watch.last_poll.elapsed() >= WATCH_EVERY {
                self.watch.last_poll = Instant::now();
                self.poll_replays();
            }
            let frame = self.logo_frame();
            if let Some(Item::Header { logo, .. }) = self.transcript.first_mut()
                && *logo != frame
            {
                *logo = frame;
                dirty = true;
            }
            if let Some(job) = &self.job {
                if let Some(who) = job.label.strip_prefix("Asking ")
                    && let Some(Panel::Review(p)) = &mut self.panel
                {
                    p.asking(Some((who.to_owned(), job.started)));
                }
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
                Event::Paste(s) => match &mut self.settings {
                    Some(p) => p.paste(&s),
                    None => self.editor.insert_str(&s),
                },
                _ => {}
            }
        }
    }

    /// What a review asks for: a clip folder, or the AI review.
    fn review_action(&mut self, a: ReviewAction) {
        let Some(Panel::Review(p)) = &self.panel else { return };
        if self.job.is_some() {
            return;
        }
        let dir = self.data_dir.clone();
        let id = p.match_id().to_owned();
        match a {
            ReviewAction::SetClips(path) => {
                if let Ok(mut cfg) = Config::load(&dir) {
                    cfg.clips_dir = Some(path);
                    let _ = cfg.save(&dir);
                }
                self.start("Finding your clips", false, move || {
                    panel_or_error(|| crate::load_review(&dir, Some(id)), |d| Flow::Review(Box::new(d)))
                });
            }
            ReviewAction::RunAi => {
                let data = p.data();
                let cfg = Config::load(&dir).unwrap_or_default();
                let Some(provider) = data.ai_provider else { return };
                let debrief = data.debrief.clone();
                let shots = data.shots();
                self.start(&format!("Asking {}", provider.name), false, move || {
                    let r = ai::run(
                        &dir,
                        provider,
                        cfg.ai_key.as_deref(),
                        cfg.ai_model.as_deref(),
                        &debrief,
                        &shots,
                    );
                    Flow::AiDone(r.map_err(|e| format!("{e:#}")))
                });
            }
            _ => {}
        }
    }

    /// Handle one key. False to quit.
    fn key(&mut self, key: KeyEvent) -> bool {
        if let Some(panel) = &mut self.panel {
            match panel.key(key) {
                PanelAction::None => {}
                PanelAction::Close => self.panel = None,
                PanelAction::Review(a) => self.review_action(a),
                PanelAction::Focus(slug) => {
                    if let Ok(mut cfg) = Config::load(&self.data_dir) {
                        cfg.focus = Some(slug.to_owned());
                        let _ = cfg.save(&self.data_dir);
                    }
                }
            }
            return true;
        }
        if let Some(panel) = &mut self.settings {
            match panel.key(key) {
                SettingsAction::None => {}
                SettingsAction::Close => {
                    self.settings = None;
                    self.refresh();
                }
                SettingsAction::Changed => self.refresh(),
            }
            return true;
        }
        if let Some(panel) = &mut self.help {
            match panel.key(key, &self.ctx.commands) {
                HelpAction::None => {}
                HelpAction::Close => self.help = None,
                HelpAction::Use(line) => {
                    self.help = None;
                    self.editor.set_line(&line);
                }
            }
            return true;
        }
        // Help opens even while a command runs.
        if key.code == KeyCode::Enter && self.job.is_some() && is_help(&self.editor.line()) {
            self.editor.set_line("");
            self.help = Some(HelpPanel::default());
            return true;
        }
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
                if is_help(&line) {
                    self.help = Some(HelpPanel::default());
                    return true;
                }
                self.transcript.push(Item::Echo(line.clone()));
                self.scroll = 0;
                let name = line.split_whitespace().next().unwrap_or(&line);
                let scans = name.trim_start_matches('/').eq_ignore_ascii_case("scan");
                let label = format!("Running {name}");
                let dir = self.data_dir.clone();
                self.start(&label, scans, move || execute_line(&line, &dir));
            }
        }
        true
    }

    /// Redraw the whole screen: transcript on top, then (bottom-up) the
    /// footer, a rule, the input, a rule, and the menu or the status line.
    fn draw(&mut self, out: &mut impl Write) -> io::Result<()> {
        let (w, h) = terminal::size().map(|(w, h)| (w as usize, h as usize))?;
        let (w, h) = (w.max(20), h.max(8));

        if let Some(panel) = &mut self.panel {
            let rows = panel.rows(w, h);
            return self.paint(out, w, h, &rows, None);
        }
        let panel_h = (h * 4 / 5).max(14).min(h);
        if let Some(panel) = &mut self.help {
            let bottom = panel.rows(&self.ctx.commands, w, panel_h);
            return self.paint(out, w, h, &bottom, None);
        }
        if let Some(panel) = &mut self.settings {
            let bottom = panel.rows(w, panel_h);
            return self.paint(out, w, h, &bottom, None);
        }

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
        self.paint(out, w, h, &bottom, Some((cursor_col, h - 3)))
    }

    /// The transcript in the rows `bottom` leaves, then `bottom`; the cursor
    /// at `cursor` or hidden.
    fn paint(
        &mut self,
        out: &mut impl Write,
        w: usize,
        h: usize,
        bottom: &[Row],
        cursor: Option<(usize, usize)>,
    ) -> io::Result<()> {
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
        if let Some((x, y)) = cursor {
            queue!(out, MoveTo(x as u16, y as u16), Show)?;
        }
        out.flush()
    }

    /// Above the box: what is running or how far back you scrolled, and
    /// messages on the right, most important first, as many as fit.
    fn status_row(&self, w: usize) -> Row {
        let mut row = Row::new();
        if let Some(job) = &self.job {
            let secs = job.started.elapsed().as_secs();
            let (style, glyph) = spike(job.started.elapsed());
            row.push((style, format!("  {glyph} ")));
            row.push((Style::fg(ACCENT), format!("{}… ", job.label)));
            row.push((Style::fg(GREY), format!("({secs}s)")));
        } else if self.scroll > 0 {
            row.push((Style::fg(GREY), format!("  ↓ {} more lines · PgDn", self.scroll)));
        }
        let messages = self
            .update_note
            .map(|n| {
                let (c, t) = n.text();
                (c, t.to_owned())
            })
            .into_iter()
            .chain(self.notices.iter().cloned());
        status_right(row, messages, w)
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
            Some(placeholder(self.ranked).to_owned())
        } else {
            self.editor.ghost(&self.ctx)
        };
        if let Some(g) = ghost {
            row.push((Style::fg(GREY), g));
        }
        (row, prompt_w + cursor - start)
    }
}

/// What the empty input line suggests.
fn placeholder(ranked: bool) -> &'static str {
    if ranked {
        "Try /review to see what problems were found from your latest match"
    } else {
        "Set your rank in /settings for a more tailored experience"
    }
}

/// `left`, then the messages that fit beside it, right-aligned.
fn status_right(mut row: Row, messages: impl Iterator<Item = (Color, String)>, w: usize) -> Row {
    let lw = row_width(&row);
    let mut right = Row::new();
    for (color, text) in messages {
        let mut piece: Row = Vec::new();
        if !right.is_empty() {
            piece.push((Style::fg(GREY), " · ".into()));
        }
        piece.push((Style::fg(color), text));
        if lw + row_width(&right) + row_width(&piece) + 3 > w {
            break;
        }
        right.extend(piece);
    }
    let rw = row_width(&right);
    if rw > 0 {
        row.push((Style::default(), " ".repeat(w - lw - rw - 2)));
        row.extend(right);
    }
    row
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) fg: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) bold: bool,
}

impl Style {
    pub(crate) fn fg(c: Color) -> Self {
        Style {
            fg: Some(c),
            ..Style::default()
        }
    }

    pub(crate) fn bold(self) -> Self {
        Style { bold: true, ..self }
    }
}

/// One screen row as styled pieces.
pub(crate) type Row = Vec<(Style, String)>;

pub(crate) fn row_width(row: &Row) -> usize {
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
            Item::Header {
                rank,
                game,
                folder,
                logo,
            } => {
                rows.push(vec![]);
                let text: [Row; 3] = [
                    vec![
                        (Style::default().bold(), APP_NAME.into()),
                        (Style::fg(GREY), format!(" v{}", crate::VERSION)),
                    ],
                    vec![
                        (
                            Style::fg(rank_color(rank.as_deref())),
                            rank.clone().unwrap_or_else(|| "Unranked".into()),
                        ),
                        (Style::fg(GREY), format!(" · {game}")),
                    ],
                    vec![(Style::fg(GREY), folder.clone())],
                ];
                for (logo, text) in logo::rows(*logo).into_iter().zip(text) {
                    let mut row: Row = vec![(Style::default(), " ".into())];
                    row.extend(logo);
                    row.push((Style::default(), "   ".into()));
                    row.extend(text);
                    rows.push(row);
                }
                rows.push(vec![]);
            }
            Item::Line(kind, text) => {
                for piece in wrap(text, text_w) {
                    rows.push(vec![(kind_style(*kind), format!(" {piece}"))]);
                }
            }
            Item::Row(pieces) => {
                let mut row: Row = vec![(Style::default(), " ".into())];
                row.extend(pieces.iter().map(|(kind, text)| (kind_style(*kind), text.clone())));
                rows.push(row);
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

pub(crate) fn kind_style(kind: Kind) -> Style {
    match kind {
        Kind::Out => Style::default(),
        Kind::Warn => Style::fg(YELLOW),
        Kind::Error => Style::fg(RED),
        Kind::Dim => Style::fg(GREY),
        Kind::Bold => Style::default().bold(),
        Kind::Good => Style::fg(crate::review_view::GREEN),
        Kind::Paint(c, true) => Style::fg(c).bold(),
        Kind::Paint(c, false) => Style::fg(c),
    }
}

/// Wrap at spaces into rows of at most `w` chars, breaking a longer word
/// where it must. Continuation rows keep the line's indent.
pub(crate) fn wrap(text: &str, w: usize) -> Vec<String> {
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
pub(crate) fn highlight(text: &str, query: &str, words: bool, base: Style) -> Row {
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

/// Beside the logo: the rank, the VALORANT version, then the replay folder.
fn header_text(data_dir: &Path) -> (Option<String>, String, String) {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let game = Catalog::load(&paths::catalog_path(data_dir))
        .ok()
        .and_then(|c| c.game_version)
        .map_or_else(
            || "VALORANT version unknown".to_owned(),
            |v| format!("VALORANT v{}", v.splitn(3, '.').take(2).collect::<Vec<_>>().join(".")),
        );
    let folder = cfg
        .demos_dir()
        .map_or_else(|| "no replay folder set".to_owned(), |d| d.display().to_string());
    (cfg.rank, game, folder)
}

/// Setup problems for the right of the status line. A patch vrfkit can't read
/// yet is said only while there's no update: an update brings the new reader.
fn notices(data_dir: &Path, update: bool) -> Vec<(Color, String)> {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let mut out = Vec::new();
    if Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()).is_err() {
        out.push((RED, "Replay reader not installed · /update installs it".to_owned()));
    }
    if vantage_core::clips::ffmpeg().is_none() {
        out.push((RED, "Screenshot tool not installed · /update installs it".to_owned()));
    }
    if !update
        && let Ok(c) = Catalog::load(&paths::catalog_path(data_dir))
        && let Some(v) = c.game_version.as_deref().filter(|v| check_game_version(v).is_some())
    {
        let patch = v.splitn(3, '.').take(2).collect::<Vec<_>>().join(".");
        out.push((
            YELLOW,
            format!("VALORANT v{patch} isn't supported · wait for an update"),
        ));
    }
    out
}

/// The agent, map and weapon names from valorant-api.com, saved to `path`.
fn fetch_catalog(path: &Path) -> Result<()> {
    vantage_core::catalog::fetch()?.save(path)
}

/// One "what's new" block: heading, bullets, then the release link.
fn whats_new(title: String, notes: &[String], link: String) {
    out::row(vec![
        (Kind::Paint(crate::review_view::GREEN, true), "✓ ".into()),
        (Kind::Bold, title),
        (Kind::Dim, " · what's new".into()),
    ]);
    for n in notes {
        out::row(vec![
            (Kind::Paint(ACCENT, false), "  • ".into()),
            (Kind::Out, n.clone()),
        ]);
    }
    out::emit(Kind::Dim, format!("  {link} for details"));
    outln!();
}

/// On launch: what's new after an update, the scan, and the focus.
fn launch(data_dir: &Path) {
    if let Some(w) = WhatsNew::take(data_dir, crate::VERSION) {
        whats_new(
            format!("{APP_NAME} updated to {}", w.version),
            &w.notes,
            format!("{}/v{}", update::RELEASES, w.version),
        );
        if let Some(v) = &w.vrfkit {
            whats_new(
                format!("vrfkit updated to {}", v.tag.trim_start_matches('v')),
                &v.notes,
                format!("{}/{}", update::VRFKIT_RELEASES, v.tag),
            );
        }
    }
    let cfg = Config::load(data_dir).unwrap_or_default();
    if Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()).is_err() {
        return;
    }
    match cfg.demos_dir() {
        Some(d) if d.is_dir() => {
            if let Err(e) = crate::scan_cmd(data_dir, None, false, false) {
                out::emit(Kind::Error, format!("Couldn't read your replays: {e:#}"));
            }
            if cfg.me.is_none() {
                detect_me(data_dir);
            }
        }
        // Matches stored earlier still count without the folder.
        _ if !crate::my_matches(data_dir).is_empty() => {}
        _ => {
            outln!("No replays yet. Download a match in the VALORANT client and it shows up here.");
            return;
        }
    }
    if cfg.rank.is_some() && !crate::my_matches(data_dir).is_empty() {
        outln!();
        crate::focus_line(data_dir);
    }
}

/// In the background on launch: install a newer release (it applies on the
/// next launch), or only say it exists when automatic updates are off or this
/// exe was not installed by the installer (a dev build). Offline or failing
/// checks stay quiet.
fn check_for_update(data_dir: &Path, tx: &Sender<UpdateNote>, install_it: bool) {
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
    let Some(layout) = installed.filter(|_| install_it) else {
        update::State::record(data_dir, format!("{v} available"));
        let _ = tx.send(UpdateNote::Available);
        return;
    };
    match update::apply(&layout, &manifest, data_dir) {
        Ok(_) => {
            update::State::record(data_dir, format!("installed {v}"));
            let _ = tx.send(UpdateNote::Installed);
        }
        Err(e) => {
            update::State::record(data_dir, format!("failed: {e:#}"));
            let _ = tx.send(UpdateNote::Failed);
        }
    }
}

/// Set `me` once exactly one account is in every scanned match (2+).
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
        dim(&format!(
            "Found your account: it's in all {n} matches. Change it in /settings if that's wrong."
        ));
    }
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

/// My matches for completing `--match` and `--map`: when, where, and how it went.
fn matches(data_dir: &Path) -> Vec<MatchInfo> {
    crate::my_matches(data_dir)
        .into_iter()
        .map(|m| {
            let map = m.map.as_deref().map(short_map).unwrap_or("?").to_owned();
            let when = m
                .played_at
                .as_deref()
                .map(crate::short_when)
                .unwrap_or_else(|| "?".into());
            let about = match &m.agent {
                Some(a) => {
                    let (res, _) = crate::review_view::result_style(m.won, m.lost);
                    format!("{a} · {res} {}–{}", m.won, m.lost)
                }
                None => "you aren't in this one".into(),
            };
            MatchInfo {
                id: m.id,
                label: format!("{when}  {map:<6}"),
                about,
                map,
            }
        })
        .collect()
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
        _ => {}
    }
    let cli = match Cli::try_parse_from(std::iter::once("vantage".to_owned()).chain(words)) {
        Ok(cli) => cli,
        Err(e) => {
            // Also how `--help` prints.
            let kind = if e.use_stderr() { Kind::Error } else { Kind::Out };
            out::show(kind, e.render().to_string().trim_end().to_owned());
            return Flow::Continue;
        }
    };
    let Some(command) = cli.command else {
        return Flow::Continue;
    };
    crate::session_log::write("CMD", command.name());
    let dir = cli.data_dir.unwrap_or_else(|| data_dir.to_path_buf());
    // In the shell, these open a panel instead of printing.
    match command {
        crate::Command::Settings => Flow::Settings,
        crate::Command::Review { match_id } => {
            panel_or_error(|| crate::load_review(&dir, match_id), |d| Flow::Review(Box::new(d)))
        }
        crate::Command::Plan { map } => panel_or_error(|| crate::load_plan(&dir, map), |d| Flow::Plan(Box::new(d))),
        crate::Command::Habits => panel_or_error(
            || crate::load_habits(&dir),
            |(p, focus, n)| Flow::Habits(Box::new(HabitsPanel::new(p, focus, n))),
        ),
        crate::Command::Rank => panel_or_error(
            || crate::load_rank(&dir),
            |(rank, standing, focus)| Flow::Rank(Box::new(RankPanel::new(rank, standing, focus))),
        ),
        command => {
            if let Err(e) = crate::run_command(&dir, command) {
                out::emit(Kind::Error, format!("{e:#}"));
            }
            Flow::Continue
        }
    }
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
#[path = "docs_pictures.rs"]
mod docs_pictures;

#[cfg(test)]
mod tests {
    use super::*;

    fn text(row: &Row) -> String {
        row.iter().map(|(_, s)| s.as_str()).collect()
    }

    #[test]
    fn labels() {
        assert_eq!(short_map("/Game/Maps/Ascent/Ascent"), "Ascent");
        assert_eq!(short_map("Lotus"), "Lotus");
        assert_eq!(
            placeholder(false),
            "Set your rank in /settings for a more tailored experience"
        );
    }

    #[test]
    fn words() {
        assert_eq!(split_words("/review  --match ab"), ["/review", "--match", "ab"]);
        assert_eq!(
            split_words(r#"/scan --demos-dir "C:\My Demos""#),
            ["/scan", "--demos-dir", r"C:\My Demos"]
        );
        assert!(split_words("   ").is_empty());
    }

    #[test]
    fn every_command_is_listed_with_its_flags() {
        let cmds = commands();
        let names: Vec<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
        for n in [
            "review", "plan", "habits", "focus", "rank", "matches", "scan", "settings", "logs", "help", "exit",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        // Removed for the coach, or automatic in the shell.
        for n in [
            "export", "summary", "trends", "init", "players", "sync", "update", "install",
        ] {
            assert!(!names.contains(&n), "{n}");
        }
        assert!(cmds.iter().all(|c| !c.about.is_empty()));
        let review = cmds.iter().find(|c| c.name == "review").unwrap();
        let m = review.flags.iter().find(|f| f.long == "--match").unwrap();
        assert_eq!(m.value.as_deref(), Some("MATCH"));
        let matches = cmds.iter().find(|c| c.name == "matches").unwrap();
        assert!(matches.flags.iter().any(|f| f.long == "--all" && f.value.is_none()));
        assert!(matches.flags.iter().all(|f| f.long != "--data-dir"));
    }

    #[test]
    fn spike_beeps_faster_the_longer_a_job_runs() {
        let beeps = |from: u64| {
            (0..50)
                .filter(|i| spike(Duration::from_millis(from * 1000 + i * 80)).0 == Style::fg(BRIGHT))
                .count()
        };
        // 4 s of ticks: every 15th at first, every 3rd once the job is long.
        assert_eq!(beeps(0), 4);
        assert_eq!(beeps(120), 17);
    }

    #[test]
    fn status_bar_keeps_what_fits_most_important_first() {
        let msgs = || {
            vec![
                (ACCENT, "Update available! Restart to apply".to_owned()),
                (
                    YELLOW,
                    "VALORANT v13.10 isn't supported · wait for an update".to_owned(),
                ),
            ]
            .into_iter()
        };
        let wide = text(&status_right(Row::new(), msgs(), 140));
        assert!(wide.contains("Restart to apply") && wide.contains("isn't supported"));
        let narrow = text(&status_right(Row::new(), msgs(), 45));
        assert!(narrow.contains("Restart to apply") && !narrow.contains("isn't supported"));
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
                rank: Some("Gold 2".into()),
                game: "VALORANT v13.08".into(),
                folder: "C:\\Demos".into(),
                logo: logo::FRAMES - 1,
            }],
            80,
        );
        assert_eq!(rows.len(), 5);
        assert!(text(&rows[1]).contains("Vantage v"));
        assert!(text(&rows[2]).contains("Gold 2 · VALORANT v13.08"));
        let rows = transcript_rows(
            &[Item::Header {
                rank: None,
                game: "VALORANT v13.08".into(),
                folder: "C:\\Demos".into(),
                logo: logo::FRAMES - 1,
            }],
            80,
        );
        assert!(text(&rows[2]).contains("Unranked · VALORANT v13.08"));
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
