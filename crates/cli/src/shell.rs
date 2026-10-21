//! `vantage` with no command: the full-screen shell, built on Abalone.
//! Vantage supplies its commands (from clap), what a line runs, its header,
//! notices, panels, settings and colours; Abalone draws the screen, edits
//! the line and runs commands on a worker thread.
//!
//! Alongside commands it watches the replay folder for new downloads, and
//! checks for a newer Vantage and vrfkit now and then.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use abalone::update::Updater;
use abalone::{
    App, CommandInfo, Completer, Ctx, Help, Notice, PanelEvent, Shell, Suggestion, Theme, UpdateNote, split_words,
};
pub(crate) use abalone::{Row, Style, wrap};
use anyhow::Result;
use clap::{CommandFactory, Parser};
use crossterm::event::KeyEvent;
use crossterm::style::Color;
use vantage_core::Catalog;
use vantage_core::ai;
use vantage_core::builds::check_game_version;
use vantage_core::config::Config;
use vantage_core::install::{self, Layout};
use vantage_core::paths;
use vantage_core::store::Store;
use vantage_core::update::{self, WhatsNew, news};
use vantage_core::vrfkit::{self, Pin, Vrfkit};

use crate::coach_views::{CoachAction, HabitsPanel, PlanData, PlanPanel, RankPanel};
use crate::logo::Chevrons;
use crate::out::{self, Kind};
use crate::panel::rank_color;
use crate::review_view::{ReviewAction, ReviewPanel};
use crate::settings::VantageSettings;
use crate::{APP_NAME, Cli, ReviewData, VERSION};

const TICK: Duration = Duration::from_millis(80);
/// How often the replay folder is checked for new downloads.
const WATCH_EVERY: Duration = Duration::from_secs(3);
/// A catalog this many days old is refreshed in the background on launch.
const SYNC_AFTER_DAYS: i64 = 1;
/// How often an open shell checks for updates again after launch.
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);

/// The working spinner: a diamond that beeps like the planted spike, once
/// every 1.2 s at first and quicker the longer the job runs, down to 0.24 s.
pub(crate) fn spike(elapsed: Duration) -> (Style, &'static str) {
    let tick = elapsed.as_millis() / TICK.as_millis();
    let gap = 15u128.saturating_sub(elapsed.as_secs() as u128 / 4).max(3);
    match tick % gap {
        // The terminal's own text colour, so the flash shows on light backgrounds too.
        0 => (Style::default().bold(), "◆"),
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
pub(crate) const BRIGHT: Color = Color::Rgb { r: 240, g: 240, b: 240 };

/// Vantage's colours, for everything Abalone draws.
pub(crate) const THEME: Theme = Theme {
    accent: ACCENT,
    bright: BRIGHT,
    muted: GREY,
    rule: Color::Rgb { r: 110, g: 110, b: 110 },
    danger: RED,
    warn: YELLOW,
    good: crate::review_view::GREEN,
    echo_bg: Color::Rgb { r: 45, g: 45, b: 45 },
    on_bright: Color::Black,
};

const ISSUES: &str = "https://github.com/Deaadman/vantage/issues";
const HOME: &str = "https://github.com/Deaadman/vantage";

/// The General tab of `/help`; Abalone adds the keys after it.
const GENERAL: &str = "Vantage watches your VALORANT replays and coaches you: why you died, what to fix, and what works at your rank. Everything stays on this computer unless you turn on AI review.

# After a match
  Download the replay in the client. Vantage picks it up and tells you how it went.
  /review opens the debrief: the short version, the issues that cost you the most rounds, what to keep doing, and how your focus habit went.
  Its Clips and Screenshots tabs match your clips (Medal, ShadowPlay, Game Bar and others) to your deaths and take a screenshot of each one.
  The AI tab in /review can ask an AI (Claude, ChatGPT, Gemini, or Ollama on your own PC) to look at those screenshots for you: where you stood and where your crosshair was. It's optional and saved once per match.

# Before you queue
  /plan gives you a game plan for a map: where you win, where you lose, and what to do on each side.
  /rank says what wins games at your rank. Set your rank in /settings; replays don't include it.

# Over time
  /habits names the mistakes you keep making and shows which are getting better.
  /focus picks one of them to work on. Every debrief checks it.";

pub fn run(data_dir: &Path) -> Result<()> {
    build(data_dir).run()
}

/// Tells Abalone about a light background set in /settings; true when set.
fn apply_light(data_dir: &Path) -> bool {
    let light = Config::load(data_dir).ok().and_then(|c| c.light);
    if let Some(l) = light {
        abalone::set_light_background(l);
    }
    light.is_some()
}

/// The shell, ready to run.
pub(crate) fn build(data_dir: &Path) -> Shell<Vantage> {
    apply_light(data_dir);
    let app = Vantage::new(data_dir);
    let (known, pin_tx) = (app.update_known.clone(), app.pin_tx.clone());
    let dir = data_dir.to_path_buf();
    let mut shell = Shell::new(app)
        .title(APP_NAME)
        .theme(THEME.for_terminal())
        .logo(Chevrons)
        .spinner(|t: Duration| {
            let (style, glyph) = spike(t);
            (style, glyph.to_owned())
        })
        .tick(TICK)
        .help(
            Help::new()
                .text("General", GENERAL)
                .commands("Commands")
                .link(HOME)
                .issues(ISSUES),
        )
        .history(data_dir.join("history.txt"))
        .tui_file(data_dir.join("tui.txt"))
        .release_notes(include_str!("../../../CHANGELOG.md"))
        .settings(move || VantageSettings::new(&dir));
    if let Ok(updater) = update::updater(data_dir, VERSION) {
        let dir = data_dir.to_path_buf();
        shell = shell.update_check(RECHECK, "/update", move |note| {
            check_for_updates(&updater, &dir, &pin_tx, note).inspect(|_| known.store(true, Ordering::Relaxed))
        });
    }
    shell
}

/// What a command's worker thread hands back.
pub(crate) enum Flow {
    Continue,
    /// Start the launch scan (after the first-run fetch).
    Launch,
    /// Open a panel.
    Review(Box<ReviewData>),
    Plan(Box<PlanData>),
    Habits(Box<HabitsPanel>),
    Rank(Box<RankPanel>),
    /// Open the settings panel.
    Settings,
    /// The review again after picking a clip folder: replaces the open one.
    Reloaded(Box<ReviewData>),
    /// The AI review of a match finished.
    AiDone(String, Result<ai::Saved, String>),
    /// The watcher stored new matches: open panels built from all matches update.
    NewMatches,
    /// Those panels again, replacing the open one of the same kind.
    Updated(Panel),
}

/// What covers the whole screen.
pub(crate) enum Panel {
    Review(ReviewPanel),
    Plan(PlanPanel),
    Habits(Box<HabitsPanel>),
    Rank(Box<RankPanel>),
}

/// What a panel's key asks Vantage to do.
pub(crate) enum PanelAction {
    Review(ReviewAction),
    Focus(&'static str),
}

impl abalone::Panel for Panel {
    type Event = PanelAction;

    fn key(&mut self, key: KeyEvent) -> PanelEvent<PanelAction> {
        let coach = |a: CoachAction| match a {
            CoachAction::None => PanelEvent::None,
            CoachAction::Close => PanelEvent::Close,
            CoachAction::Focus(s) => PanelEvent::Event(PanelAction::Focus(s)),
        };
        match self {
            Panel::Review(p) => match p.key(key) {
                ReviewAction::None => PanelEvent::None,
                ReviewAction::Close => PanelEvent::Close,
                a => PanelEvent::Event(PanelAction::Review(a)),
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

/// A stored match, for completing `--match`.
#[derive(Debug, Clone)]
pub(crate) struct MatchInfo {
    pub id: String,
    /// `Mon 19 Oct 21:04  Ascent`.
    pub label: String,
    /// `Sova · won 13–8`.
    pub about: String,
    /// `Ascent`, for completing `--map`.
    pub map: String,
}

/// Values for the command menu: matches, maps and habits.
#[derive(Default)]
pub(crate) struct Completions {
    pub matches: Vec<MatchInfo>,
}

impl Completer for Completions {
    fn complete(&self, cmd: &str, flag: Option<&str>, args: &[&str], word: &str) -> Vec<Suggestion> {
        match (cmd, flag) {
            (_, Some("--match")) => self.match_items(word, ""),
            (_, Some("--map")) => self.map_items(word, ""),
            ("focus", None) if args.is_empty() => habit_items(word),
            // A bare command offers what it is about, then its flags.
            ("review", None) if args.is_empty() && word.is_empty() => self.match_items("", "--match "),
            ("plan", None) if args.is_empty() && word.is_empty() => self.map_items("", "--map "),
            _ => vec![],
        }
    }
}

impl Completions {
    /// Maps with stored matches, most played first.
    fn map_items(&self, prefix: &str, value_prefix: &str) -> Vec<Suggestion> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for m in self.matches.iter().filter(|m| m.map != "?") {
            match counts.iter_mut().find(|(name, _)| *name == m.map) {
                Some((_, n)) => *n += 1,
                None => counts.push((m.map.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let prefix = prefix.to_lowercase();
        counts
            .into_iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&prefix))
            .map(|(name, n)| {
                Suggestion::new(
                    format!("{value_prefix}{name}"),
                    name,
                    format!("{n} match{}", if n == 1 { "" } else { "es" }),
                )
            })
            .collect()
    }

    /// Matches by when, where and how they went; the id is only what gets typed.
    fn match_items(&self, prefix: &str, value_prefix: &str) -> Vec<Suggestion> {
        self.matches
            .iter()
            .filter(|m| m.id.starts_with(prefix))
            .map(|m| {
                Suggestion::new(
                    format!("{value_prefix}{}", m.id.chars().take(8).collect::<String>()),
                    m.label.clone(),
                    m.about.clone(),
                )
            })
            .collect()
    }
}

fn habit_items(prefix: &str) -> Vec<Suggestion> {
    let p = prefix.to_lowercase();
    vantage_core::habits::HABITS
        .iter()
        .filter(|h| h.slug.starts_with(&p))
        .map(|h| Suggestion::new(h.slug, h.slug, h.name))
        .collect()
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

pub(crate) struct Vantage {
    data_dir: PathBuf,
    completions: Completions,
    /// Beside the logo: the rank, the VALORANT version, the replay folder.
    header: (Option<String>, String, String),
    notices: Vec<Notice>,
    /// Whether a rank is set, for the input's placeholder.
    ranked: bool,
    watch: Watch,
    /// Set by the update check once it has news: the shell shows it, so
    /// the notices leave out what an update would fix.
    update_known: Arc<AtomicBool>,
    seen_update: bool,
    /// vrfkit releases the update check installed.
    pin_tx: Sender<Pin>,
    pin_rx: Receiver<Pin>,
    /// A vrfkit that was just installed, announced once no job is running.
    new_vrfkit: Option<Pin>,
    /// A review's request made while a job ran, started when it ends.
    queued: Option<(String, ReviewAction)>,
}

impl Vantage {
    fn new(data_dir: &Path) -> Self {
        let (pin_tx, pin_rx) = mpsc::channel();
        let mut app = Vantage {
            data_dir: data_dir.to_path_buf(),
            completions: Completions::default(),
            header: (None, String::new(), String::new()),
            notices: vec![],
            ranked: false,
            watch: Watch {
                last_poll: Instant::now(),
                seen: HashMap::new(),
                pending: HashMap::new(),
            },
            update_known: Arc::new(AtomicBool::new(false)),
            seen_update: false,
            pin_tx,
            pin_rx,
            new_vrfkit: None,
            queued: None,
        };
        app.refresh();
        app
    }

    /// Run `job` on a worker thread. A job that `scans` marks the replay
    /// folder's current files as seen, so the watcher only picks up later ones.
    fn start(&mut self, ctx: &mut Ctx<Self>, label: &str, scans: bool, job: impl FnOnce() -> Flow + Send + 'static) {
        if scans {
            self.watch.seen = replay_sizes(&self.data_dir);
            self.watch.pending.clear();
        }
        ctx.start(label, job);
    }

    /// The launch: what's new after an update, the scan, and the focus.
    fn start_launch(&mut self, ctx: &mut Ctx<Self>) {
        let dir = self.data_dir.clone();
        self.start(ctx, "Checking for new replays", true, move || {
            launch(&dir);
            Flow::Continue
        });
    }

    /// Scan replays that appeared or changed since the last scan, once their
    /// size holds still for a poll (a download in progress keeps growing).
    fn poll_replays(&mut self, ctx: &mut Ctx<Self>) {
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
        // The scan skips files changed moments ago; wait until it will read them.
        if !changed.keys().all(|p| vantage_core::scan::settled(p)) {
            return;
        }
        let dir = self.data_dir.clone();
        self.start(ctx, "Reading the new replay", true, move || {
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
            if stored.is_empty() {
                Flow::Continue
            } else {
                Flow::NewMatches
            }
        });
    }

    /// What a review asks for: a clip folder, or the AI review.
    fn review_action(&mut self, a: ReviewAction, ctx: &mut Ctx<Self>) {
        let busy = ctx.busy();
        let Some(Panel::Review(p)) = ctx.panel() else { return };
        // One job at a time: this one starts when the running one ends.
        if busy {
            if matches!(a, ReviewAction::SetClips(_) | ReviewAction::RunAi) {
                p.note("Starting as soon as Vantage finishes what it's doing…".into());
                self.queued = Some((p.match_id().to_owned(), a));
            }
            return;
        }
        let dir = self.data_dir.clone();
        let id = p.match_id().to_owned();
        let data = p.data();
        let ai = data
            .ai_provider
            .map(|provider| (provider, data.debrief.clone(), data.shots()));
        match a {
            ReviewAction::SetClips(path) => {
                let saved = Config::load(&dir).and_then(|mut cfg| {
                    cfg.clips_dir = Some(path);
                    cfg.save(&dir)
                });
                if let Err(e) = saved {
                    p.note(format!("Couldn't save the clip folder: {e:#}"));
                    return;
                }
                self.start(ctx, "Finding your clips", false, move || {
                    panel_or_error(|| crate::load_review(&dir, Some(id)), |d| Flow::Reloaded(Box::new(d)))
                });
            }
            ReviewAction::RunAi => {
                let cfg = Config::load(&dir).unwrap_or_default();
                let Some((provider, debrief, shots)) = ai else { return };
                self.start(ctx, &format!("Asking {}", provider.name), false, move || {
                    let r = ai::run(
                        &dir,
                        provider,
                        cfg.ai_key.as_deref(),
                        cfg.ai_model.as_deref(),
                        &debrief,
                        &shots,
                    );
                    Flow::AiDone(debrief.match_id.clone(), r.map_err(|e| format!("{e:#}")))
                });
            }
            _ => {}
        }
    }
}

impl App for Vantage {
    type Msg = Flow;
    type Panel = Panel;

    /// The clap subcommands and their long flags (the shell adds help, clear
    /// and exit), less `uninstall`, which can't remove the running shell.
    fn commands(&self) -> Vec<CommandInfo> {
        let mut cmds = CommandInfo::from_clap(&Cli::command());
        cmds.retain(|c| c.name != "uninstall");
        cmds
    }

    fn run(&mut self, line: &str, ctx: &mut Ctx<Self>) {
        let name = line.split_whitespace().next().unwrap_or(line);
        let scans = name.trim_start_matches('/').eq_ignore_ascii_case("scan");
        let label = format!("Running {name}");
        let (line, dir) = (line.to_owned(), self.data_dir.clone());
        self.start(ctx, &label, scans, move || execute_line(&line, &dir));
    }

    fn launch(&mut self, ctx: &mut Ctx<Self>) {
        // With no catalog yet, the launch fetches one first; an old one refreshes quietly.
        let path = paths::catalog_path(&self.data_dir);
        let stale = Catalog::load(&path).ok().is_some_and(|c| {
            c.age_days(vantage_core::time::now_unix())
                .is_none_or(|d| d >= SYNC_AFTER_DAYS)
        });
        if stale {
            let path = path.clone();
            std::thread::spawn(move || {
                if let Ok(cat) = vantage_core::catalog::fetch() {
                    let _ = cat.save(&path);
                }
            });
        }
        if path.is_file() {
            self.start_launch(ctx);
        } else {
            // First run: names for agents, maps and weapons before anything else.
            self.start(ctx, "Fetching VALORANT data", false, move || {
                if let Err(e) = fetch_catalog(&path) {
                    errln!("Couldn't fetch VALORANT data, so built-in names are used: {e:#}");
                }
                Flow::Launch
            });
        }
    }

    fn done(&mut self, flow: Flow, ctx: &mut Ctx<Self>) {
        match flow {
            Flow::Continue => {}
            Flow::Launch => self.start_launch(ctx),
            Flow::Review(data) => ctx.open(Panel::Review(ReviewPanel::new(*data))),
            Flow::Plan(data) => ctx.open(Panel::Plan(PlanPanel::new(*data))),
            Flow::Habits(p) => ctx.open(Panel::Habits(p)),
            Flow::Rank(p) => ctx.open(Panel::Rank(p)),
            Flow::Settings => ctx.open_settings(),
            // Only while the review is still open: closing it means the player moved on.
            Flow::Reloaded(data) => {
                if let Some(Panel::Review(p)) = ctx.panel()
                    && p.match_id() == data.debrief.match_id
                {
                    let p = p.reloaded(*data);
                    ctx.open(Panel::Review(p));
                }
            }
            Flow::NewMatches => {
                let dir = self.data_dir.clone();
                match ctx.panel() {
                    Some(Panel::Habits(_)) => self.start(ctx, "Updating your habits", false, move || {
                        crate::load_habits(&dir).map_or(Flow::Continue, |(p, focus, n)| {
                            Flow::Updated(Panel::Habits(Box::new(HabitsPanel::new(p, focus, n))))
                        })
                    }),
                    Some(Panel::Rank(_)) => self.start(ctx, "Updating your rank", false, move || {
                        crate::load_rank(&dir).map_or(Flow::Continue, |(rank, standing, focus)| {
                            Flow::Updated(Panel::Rank(Box::new(RankPanel::new(rank, standing, focus))))
                        })
                    }),
                    _ => {}
                }
            }
            Flow::Updated(new) => match (ctx.panel(), new) {
                (Some(Panel::Habits(old)), Panel::Habits(new)) => {
                    let p = old.updated(*new);
                    ctx.open(Panel::Habits(Box::new(p)));
                }
                (Some(Panel::Rank(old)), Panel::Rank(new)) => {
                    let p = old.updated(*new);
                    ctx.open(Panel::Rank(Box::new(p)));
                }
                _ => {}
            },
            Flow::AiDone(id, result) => match ctx.panel() {
                Some(Panel::Review(p)) if p.match_id() == id => p.ai_done(result),
                // The review was closed meanwhile: say how it went in the transcript.
                _ => match result {
                    Ok(_) => out::emit(
                        Kind::Good,
                        "The AI review is done. Open /review and go to the AI tab.".into(),
                    ),
                    Err(e) => out::emit(Kind::Error, format!("The AI review didn't finish: {e}")),
                },
            },
        }
    }

    fn panel_event(&mut self, event: PanelAction, ctx: &mut Ctx<Self>) {
        match event {
            PanelAction::Review(a) => self.review_action(a, ctx),
            PanelAction::Focus(slug) => {
                let saved = Config::load(&self.data_dir).and_then(|mut cfg| {
                    cfg.focus = Some(slug.to_owned());
                    cfg.save(&self.data_dir)
                });
                if let (Err(e), Some(Panel::Habits(p))) = (saved, ctx.panel()) {
                    p.note(format!("Couldn't save your focus: {e:#}"));
                }
            }
        }
    }

    fn tick(&mut self, ctx: &mut Ctx<Self>) {
        while let Ok(pin) = self.pin_rx.try_recv() {
            self.new_vrfkit = Some(pin);
        }
        let known = self.update_known.load(Ordering::Relaxed);
        if known != self.seen_update {
            self.seen_update = known;
            ctx.refresh();
        }
        if !ctx.busy()
            && let Some(pin) = self.new_vrfkit.take()
        {
            vrfkit_news(&pin).into_iter().for_each(out::row);
            let dir = self.data_dir.clone();
            self.start(ctx, "Reading replays with the new vrfkit", true, move || {
                rescan(&dir);
                Flow::Continue
            });
        }
        // Only for the review it was asked from, if that's still open.
        if !ctx.busy()
            && let Some((id, a)) = self.queued.take()
            && matches!(ctx.panel(), Some(Panel::Review(p)) if p.match_id() == id)
        {
            self.review_action(a, ctx);
        }
        if !ctx.busy() && self.watch.last_poll.elapsed() >= WATCH_EVERY {
            self.watch.last_poll = Instant::now();
            self.poll_replays(ctx);
        }
        // The review shows its own spinner while the AI is asked.
        let asking = ctx
            .job()
            .and_then(|(label, started)| label.strip_prefix("Asking ").map(|who| (who.to_owned(), started)));
        if let Some(Panel::Review(p)) = ctx.panel() {
            p.asking(asking);
        }
    }

    /// Re-read what commands may have changed: matches, settings, notices.
    fn refresh(&mut self) {
        if apply_light(&self.data_dir) {
            abalone::set_theme(THEME.for_terminal());
        }
        self.completions.matches = matches(&self.data_dir);
        self.notices = notices(&self.data_dir, self.update_known.load(Ordering::Relaxed));
        self.ranked = Config::load(&self.data_dir).is_ok_and(|c| c.rank.is_some());
        self.header = header_text(&self.data_dir);
    }

    fn header(&self) -> Vec<Row> {
        let (rank, game, folder) = &self.header;
        header_lines(rank.as_deref(), game, folder)
    }

    fn notices(&self) -> Vec<Notice> {
        self.notices.clone()
    }

    fn footer(&self) -> String {
        let n = self.completions.matches.len();
        format!(
            "{n} match{} · /help for commands and keys",
            if n == 1 { "" } else { "es" }
        )
    }

    fn placeholder(&self) -> String {
        placeholder(self.ranked).into()
    }

    fn completer(&self) -> &dyn Completer {
        &self.completions
    }

    fn crashed(&self) -> String {
        format!(
            "internal error: the command crashed (please report){}",
            abalone::log::hint()
        )
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

/// Beside the logo: the name and version, the rank and VALORANT version, then the replay folder.
pub(crate) fn header_lines(rank: Option<&str>, game: &str, folder: &str) -> Vec<Row> {
    vec![
        vec![
            (Style::default().bold(), APP_NAME.into()),
            (Style::fg(GREY), format!(" v{VERSION}")),
        ],
        vec![
            (Style::fg(rank_color(rank)), rank.unwrap_or("Unranked").to_owned()),
            (Style::fg(GREY), format!(" · {game}")),
        ],
        vec![(Style::fg(GREY), folder.to_owned())],
    ]
}

/// The rank, the VALORANT version, and the replay folder.
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
fn notices(data_dir: &Path, update: bool) -> Vec<Notice> {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let mut out = Vec::new();
    if Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()).is_err() {
        out.push(Notice::danger("Replay reader not installed · /update installs it"));
    }
    if vantage_core::clips::ffmpeg().is_none() {
        out.push(Notice::danger("Screenshot tool not installed · /update installs it"));
    }
    if !update
        && let Ok(c) = Catalog::load(&paths::catalog_path(data_dir))
        && let Some(v) = c
            .game_version
            .as_deref()
            .filter(|v| check_game_version(v, &vrfkit::active().decodes()).is_some())
    {
        let patch = v.splitn(3, '.').take(2).collect::<Vec<_>>().join(".");
        out.push(Notice::warn(format!(
            "VALORANT v{patch} isn't supported · wait for an update"
        )));
    }
    out
}

/// The agent, map and weapon names from valorant-api.com, saved to `path`.
fn fetch_catalog(path: &Path) -> Result<()> {
    vantage_core::catalog::fetch()?.save(path)
}

/// One "what's new" block: a heading, the notes, then the release link. With
/// no notes it's just the heading and the link.
/// What a vrfkit update brings, from its `vrfkit.json` notes.
pub(crate) fn vrfkit_news(pin: &Pin) -> Vec<out::Line> {
    news(
        &format!("vrfkit updated to v{}", pin.tag.trim_start_matches('v')),
        &pin.notes,
        &format!("{}/{}", update::VRFKIT_RELEASES, pin.tag),
    )
}

/// What a Vantage update brings, for its first launch: its block and, when
/// it brought a newer vrfkit, one for that.
fn whats_new(w: &WhatsNew) -> Vec<out::Line> {
    let mut lines = news(
        &format!("{APP_NAME} updated to v{}", w.version),
        &w.notes,
        &format!("{}/{}", update::RELEASES, w.version),
    );
    if let Some(v) = update::vrfkit_notes(w) {
        lines.extend(news(
            &format!("vrfkit updated to v{}", v.tag.trim_start_matches('v')),
            &v.notes,
            &format!("{}/{}", update::VRFKIT_RELEASES, v.tag),
        ));
    }
    lines
}

/// After a vrfkit update: scan again, so replays the old one couldn't read are read now.
fn rescan(data_dir: &Path) {
    let cfg = Config::load(data_dir).unwrap_or_default();
    if cfg.demos_dir().is_some_and(|d| d.is_dir())
        && let Err(e) = crate::scan_cmd(data_dir, None, false, false)
    {
        out::emit(Kind::Error, format!("Couldn't read your replays: {e:#}"));
    }
}

/// On launch: what's new after an update, the scan, and the focus.
fn launch(data_dir: &Path) {
    if let Some(w) = WhatsNew::take(data_dir, VERSION) {
        whats_new(&w).into_iter().for_each(out::row);
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

/// On a worker thread, at launch and every `RECHECK`: install a newer
/// Vantage (it applies on the next launch) unless one already is, then a
/// newer vrfkit (used straight away, through `pins`). When automatic
/// updates are off or this exe was not installed by the installer (a dev
/// build), only say an update exists. Offline or failing checks stay quiet.
fn check_for_updates(
    updater: &Updater,
    data_dir: &Path,
    pins: &Sender<Pin>,
    note: Option<UpdateNote>,
) -> Option<UpdateNote> {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let install_it = updater.auto_disabled(cfg.auto_update != Some(false)).is_none();
    let installed = updater.installed().cloned();
    if let Some(layout) = &installed {
        install::cleanup(layout, VERSION);
    }
    // An installed update stays the news until the restart.
    let vantage = if note == Some(UpdateNote::Installed) {
        note
    } else {
        updater.check(install_it)
    };
    let layout = installed.filter(|_| install_it);
    let vrfkit = check_vrfkit(updater, pins, layout.as_ref());
    if vantage == Some(UpdateNote::Installed) {
        vantage
    } else {
        vrfkit.or(vantage)
    }
}

/// A newer vrfkit in `vrfkit.json` on `main`, for this version of Vantage.
fn check_vrfkit(updater: &Updater, pins: &Sender<Pin>, layout: Option<&Layout>) -> Option<UpdateNote> {
    let pin = update::latest_pin(updater).ok()?;
    if !install::is_newer(&pin.tag, &vrfkit::active().tag) || install::is_newer(&pin.min_vantage, VERSION) {
        return None;
    }
    let Some(layout) = layout else {
        return Some(UpdateNote::Available);
    };
    match install::update_vrfkit(layout, &pin, VERSION) {
        Ok(true) => {
            let _ = pins.send(pin);
            None
        }
        Ok(false) => None,
        Err(e) => {
            abalone::log::write("WARN", &format!("vrfkit update failed: {e:#}"));
            Some(UpdateNote::Failed)
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
        out::emit(
            Kind::Dim,
            format!("Found your account: it's in all {n} matches. Change it in /settings if that's wrong."),
        );
    }
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

/// Run one line (on the worker thread). The shell answers help, clear and exit itself.
fn execute_line(line: &str, data_dir: &Path) -> Flow {
    let mut words = split_words(line);
    let Some(first) = words.first_mut() else {
        return Flow::Continue;
    };
    *first = first.trim_start_matches('/').to_lowercase();
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
    abalone::log::write("CMD", command.name());
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
        crate::Command::Uninstall { .. } => {
            out::emit(
                Kind::Error,
                "To uninstall, quit Vantage and run `vantage uninstall`".into(),
            );
            Flow::Continue
        }
        command => {
            if let Err(e) = crate::run_command(&dir, command) {
                out::emit(Kind::Error, format!("{e:#}"));
            }
            Flow::Continue
        }
    }
}

#[cfg(test)]
#[path = "docs_pictures.rs"]
mod docs_pictures;

#[cfg(test)]
mod tests {
    use super::*;
    use abalone::editor::{Context, suggest};
    use abalone::row_text;

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
    fn every_command_is_listed_with_its_flags() {
        let dir = std::env::temp_dir().join(format!("vantage-shell-cmds-{}", std::process::id()));
        let shell = build(&dir);
        let cmds = shell.commands();
        let names: Vec<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
        for n in [
            "review", "plan", "habits", "focus", "rank", "matches", "scan", "settings", "logs", "update", "help",
            "exit",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        // Removed for the coach, automatic in the shell, or terminal-only.
        for n in [
            "export",
            "summary",
            "trends",
            "init",
            "players",
            "sync",
            "install",
            "uninstall",
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completes_matches_maps_and_habits() {
        let cmds = CommandInfo::from_clap(&Cli::command());
        let m = |id: &str, label: &str, map: &str| MatchInfo {
            id: id.into(),
            label: label.into(),
            about: "Sova · won 13–8".into(),
            map: map.into(),
        };
        let completions = Completions {
            matches: vec![
                m("aaaa1111-0000", "2026-10-19  Ascent", "Ascent"),
                m("bbbb2222-0000", "2026-10-18  Lotus", "Lotus"),
                m("cccc3333-0000", "2026-10-17  Lotus", "Lotus"),
            ],
        };
        let ctx = Context {
            commands: &cmds,
            completer: &completions,
        };
        let values = |s: &str| -> Vec<String> { suggest(s, &ctx).items.into_iter().map(|i| i.value).collect() };
        // A bare `/review ` offers the stored matches, then the flags.
        assert_eq!(
            values("/review "),
            ["--match aaaa1111", "--match bbbb2222", "--match cccc3333", "--match"]
        );
        assert_eq!(suggest("/review ", &ctx).items[0].label, "2026-10-19  Ascent");
        // `/plan` offers maps, most played first.
        assert_eq!(values("/plan ")[..2], ["--map Lotus", "--map Ascent"]);
        assert_eq!(suggest("/plan ", &ctx).items[0].description, "2 matches");
        assert_eq!(values("/plan --map lo"), ["Lotus"]);
        assert_eq!(values("/review --match b"), ["bbbb2222"]);
        assert!(values("/focus ").contains(&"untraded".to_owned()));
        assert!(values("/focus unt").iter().all(|v| v.starts_with("unt")));
    }

    #[test]
    fn spike_beeps_faster_the_longer_a_job_runs() {
        let beeps = |from: u64| {
            (0..50)
                .filter(|i| spike(Duration::from_millis(from * 1000 + i * 80)).0 == Style::default().bold())
                .count()
        };
        // 4 s of ticks: every 15th at first, every 3rd once the job is long.
        assert_eq!(beeps(0), 4);
        assert_eq!(beeps(120), 17);
    }

    fn shown(lines: Vec<out::Line>) -> Vec<String> {
        abalone::set_theme(THEME);
        lines
            .iter()
            .flat_map(|l| abalone::skin::pieces_rows(l, 80))
            .map(|r| row_text(&r))
            .collect()
    }

    #[test]
    fn whats_new_after_an_update() {
        let mut extra = serde_json::Map::new();
        extra.insert("vrfkit".into(), serde_json::json!({"tag": "v0.4.0", "notes": []}));
        let w = WhatsNew {
            version: "0.3.0".into(),
            notes: vec![
                "Habits by map.".into(),
                "A note long enough that it has to wrap onto a second row, under the first row's text.".into(),
            ],
            extra,
        };
        assert_eq!(
            shown(whats_new(&w)),
            [
                " ✓ Vantage updated to v0.3.0 · what's new",
                "   • Habits by map.",
                "   • A note long enough that it has to wrap onto a second row, under the first",
                "     row's text.",
                "   github.com/Deaadman/vantage/releases/tag/0.3.0 for details",
                " ",
                // vrfkit's notes are in Vantage's now, so its block is the heading and link.
                " ✓ vrfkit updated to v0.4.0",
                "   github.com/yakisoba0728/vrfkit/releases/tag/v0.4.0 for details",
                " ",
            ]
        );
    }

    #[test]
    fn whats_new_after_a_vrfkit_update() {
        let pin = Pin {
            tag: "v0.4.0".into(),
            notes: vec!["Supports VALORANT 13.10".into()],
            ..vrfkit::bundled().clone()
        };
        assert_eq!(
            shown(vrfkit_news(&pin)),
            [
                " ✓ vrfkit updated to v0.4.0 · what's new",
                "   • Supports VALORANT 13.10",
                "   github.com/yakisoba0728/vrfkit/releases/tag/v0.4.0 for details",
                " ",
            ]
        );
    }

    #[test]
    fn header_beside_the_logo() {
        use abalone::Logo;
        let rows = abalone::skin::header(
            Chevrons.rows(crate::logo::FRAMES - 1),
            &header_lines(Some("Gold 2"), "VALORANT v13.08", "C:\\Demos"),
            80,
        );
        assert_eq!(rows.len(), 5);
        assert!(row_text(&rows[1]).contains("Vantage v"));
        assert!(row_text(&rows[2]).contains("Gold 2 · VALORANT v13.08"));
        let rows = header_lines(None, "VALORANT v13.08", "C:\\Demos");
        assert!(row_text(&rows[1]).contains("Unranked · VALORANT v13.08"));
    }

    #[test]
    fn the_install_script_and_release_notes_come_from_abalone() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
        assert!(
            abalone::release::same_text(&read("install.ps1"), &install_script()),
            "install.ps1 is out of date: write install_script() to it"
        );
        assert!(
            abalone::release::same_text(&read("scripts/release-notes.ps1"), abalone::release::NOTES_SCRIPT),
            "scripts/release-notes.ps1 is out of date: copy abalone::release::NOTES_SCRIPT to it"
        );
    }

    /// `install.ps1`: Abalone's, plus ffmpeg for the screenshots.
    fn install_script() -> String {
        abalone::install::InstallScript::new(APP_NAME, install::APP, update::REPO)
            .about("It also fetches the vrfkit release Vantage reads replays with, and")
            .about("ffmpeg, which takes the screenshots, from winget if it isn't installed.")
            .after(
                r"
    # ffmpeg takes the screenshots of your deaths from your clips. Without it
    # Vantage still works, and its status bar says /update installs it.
    $wingetLinks = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links\ffmpeg.exe'
    if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue) -and -not (Test-Path $wingetLinks)) {
        if (Get-Command winget -ErrorAction SilentlyContinue) {
            Write-Host '  Installing ffmpeg for screenshots (winget)'
            winget install --id Gyan.FFmpeg --exact --silent --accept-source-agreements --accept-package-agreements | Out-Null
            if ($LASTEXITCODE -ne 0) { Write-Host '  ffmpeg could not be installed; /update tries again' }
        } else {
            Write-Host '  winget was not found, so ffmpeg was not installed; /update tries again'
        }
    }",
            )
            .render()
    }

    /// Rewrites `install.ps1` from `install_script()`.
    #[test]
    #[ignore = "rewrites install.ps1"]
    fn write_install_script() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::fs::write(root.join("install.ps1"), install_script()).unwrap();
        std::fs::write(root.join("scripts/release-notes.ps1"), abalone::release::NOTES_SCRIPT).unwrap();
    }
}
