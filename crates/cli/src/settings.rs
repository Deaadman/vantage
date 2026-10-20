//! `/settings`: what valreview runs with (Status) and what `config.toml`
//! holds (Config), drawn in place of the input box until Esc. Outside the
//! shell, `valreview settings` prints the Status tab.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use valreview_core::builds::{SUPPORTED_RANGE, check_game_version};
use valreview_core::config::Config;
use valreview_core::install::Layout;
use valreview_core::store::Store;
use valreview_core::vrfkit::{PINNED, Vrfkit};
use valreview_core::{Catalog, paths, update};

use crate::out::{self, Kind, Line};
use crate::review_view::GREEN;
use crate::shell::{ACCENT, BRIGHT, GREY, RED, Row, Style, highlight, kind_style};

const TABS: [&str; 2] = ["Status", "Config"];
const LABEL_W: usize = 34;
/// At most this many accounts to pick from.
const ACCOUNTS: usize = 8;

/// The Status tab: labelled values in groups.
pub type StatusGroups = Vec<Vec<(&'static str, Line)>>;

fn piece(kind: Kind, text: impl Into<String>) -> (Kind, String) {
    (kind, text.into())
}

/// Everything the Status tab shows. `update_note` is what the shell's
/// background update check found this session.
pub fn status_groups(data_dir: &Path, update_note: Option<&str>) -> StatusGroups {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let exe = std::env::current_exe().ok();
    let version = match Layout::new() {
        Ok(l) if exe.as_deref().is_some_and(|e| l.contains(e)) => vec![
            piece(Kind::Out, crate::VERSION),
            piece(Kind::Dim, format!(" · installed in {}", l.root.display())),
        ],
        _ => vec![
            piece(Kind::Out, crate::VERSION),
            piece(
                Kind::Dim,
                format!(
                    " · not installed, running from {}",
                    exe.as_deref().map_or("?".into(), |e| e.display().to_string())
                ),
            ),
        ],
    };
    let updates = match update::auto_disabled(cfg.auto_update) {
        Some(why) => vec![
            piece(Kind::Out, "Off"),
            piece(Kind::Dim, format!(" · {why}; /update installs a new release by hand")),
        ],
        None => {
            let detail = match (update_note, update::State::load(data_dir)) {
                (Some(note), _) => format!(" · {note}"),
                (None, Some(s)) => format!(" · last check {}: {}", short_time(&s.checked_at), s.result),
                (None, None) => " · not checked yet".to_owned(),
            };
            vec![piece(Kind::Out, "Automatic"), piece(Kind::Dim, detail)]
        }
    };
    let account = match &cfg.me {
        Some(me) => vec![piece(Kind::Out, me.clone())],
        None => vec![
            piece(Kind::Warn, "Not set"),
            piece(Kind::Dim, " · found once 2 scanned matches share one account"),
        ],
    };
    let now = valreview_core::time::now_unix();
    let game = match Catalog::load(&paths::catalog_path(data_dir)) {
        Err(_) => vec![
            piece(Kind::Warn, "Not fetched yet"),
            piece(Kind::Dim, " · fetched when the shell opens"),
        ],
        Ok(c) => {
            let v = c.game_version.clone().unwrap_or_else(|| "version unknown".into());
            let days = c.age_days(now);
            if c.game_version.as_deref().and_then(check_game_version).is_some() {
                vec![
                    piece(Kind::Warn, v),
                    piece(Kind::Dim, format!(" · newer than vrfkit reads ({SUPPORTED_RANGE})")),
                ]
            } else if let Some(d) = days.filter(|&d| d > 30) {
                vec![
                    piece(Kind::Warn, format!("{v} · {d} days old")),
                    piece(Kind::Dim, " · refreshes when the shell opens"),
                ]
            } else {
                let fetched = match days {
                    Some(0) => "fetched today".to_owned(),
                    Some(1) => "fetched yesterday".to_owned(),
                    Some(d) => format!("fetched {d} days ago"),
                    None => "fetch date unknown".to_owned(),
                };
                vec![
                    piece(Kind::Out, v),
                    piece(
                        Kind::Dim,
                        format!(
                            " · {} agents, {} maps, {} weapons · {fetched}",
                            c.agents.len(),
                            c.maps.len(),
                            c.weapons.len()
                        ),
                    ),
                ]
            }
        }
    };
    let vrfkit = match Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()) {
        Ok(v) => {
            let own = cfg.vrfkit.is_some() && !installed_path(cfg.vrfkit.as_deref());
            vec![
                piece(Kind::Out, v.version.clone()),
                piece(
                    Kind::Dim,
                    if own {
                        format!(
                            " · your own copy at {} · reads game builds {SUPPORTED_RANGE}",
                            v.path.display()
                        )
                    } else {
                        format!(" · SHA-256 verified · reads game builds {SUPPORTED_RANGE}")
                    },
                ),
            ]
        }
        Err(e) => vec![piece(Kind::Error, "Not set up"), piece(Kind::Dim, format!(" · {e:#}"))],
    };
    let db = paths::db_path(data_dir);
    let matches = match db.is_file().then(|| Store::open(&db).and_then(|s| s.counts())) {
        Some(Ok(c)) if c.matches > 0 => {
            let mut line = vec![piece(
                Kind::Out,
                format!("{} stored · {} deaths across every player", c.matches, c.deaths),
            )];
            if c.logged > 0 {
                line.push(piece(
                    Kind::Dim,
                    format!(
                        " · {} replay{} skipped (failed, unreadable or a duplicate)",
                        c.logged,
                        if c.logged == 1 { "" } else { "s" }
                    ),
                ));
            }
            line
        }
        Some(Err(e)) => vec![piece(Kind::Error, "Unreadable"), piece(Kind::Dim, format!(" · {e:#}"))],
        _ => vec![piece(Kind::Dim, "None yet")],
    };
    let replays = match cfg.demos_dir() {
        Some(d) if d.is_dir() => {
            let n = valreview_core::scan::find_replays(&d).map(|v| v.len()).unwrap_or(0);
            vec![piece(Kind::Out, format!("{n} in {}", d.display()))]
        }
        Some(d) => vec![
            piece(Kind::Warn, "Folder doesn't exist yet"),
            piece(Kind::Dim, format!(" · {}", d.display())),
        ],
        None => vec![piece(Kind::Warn, "No folder set")],
    };
    vec![
        vec![("Version", version), ("Updates", updates), ("Account", account)],
        vec![("VALORANT data", game), ("vrfkit", vrfkit)],
        vec![
            ("Matches", matches),
            ("Replays", replays),
            ("Data folder", vec![piece(Kind::Out, data_dir.display().to_string())]),
        ],
    ]
}

/// `valreview settings` outside the shell: the Status tab as text.
pub fn print_status(data_dir: &Path) {
    for (i, group) in status_groups(data_dir, None).into_iter().enumerate() {
        if i > 0 {
            outln!();
        }
        for (label, value) in group {
            let mut row = vec![piece(Kind::Out, format!("{:<17}", format!("{label}:")))];
            row.extend(value);
            out::row(row);
        }
    }
}

/// `2026-10-20T08:14:03Z` -> `2026-10-20 08:14`.
fn short_time(t: &str) -> String {
    t.get(..16).unwrap_or(t).replacen('T', " ", 1)
}

/// The vrfkit `valreview update` installs, rather than a copy of your own.
fn installed_path(p: Option<&Path>) -> bool {
    p.is_some_and(|p| Layout::new().is_ok_and(|l| p.starts_with(&l.root)))
}

/// One line of the Config tab, in alphabetical order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
    Account,
    AutoUpdate,
    DataFolder,
    KeepRaw,
    ReplayFolder,
    Vrfkit,
    Watch,
}

const SETTINGS: [Setting; 7] = [
    Setting::Account,
    Setting::AutoUpdate,
    Setting::DataFolder,
    Setting::KeepRaw,
    Setting::ReplayFolder,
    Setting::Vrfkit,
    Setting::Watch,
];

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Setting::Account => "Account",
            Setting::AutoUpdate => "Automatic updates",
            Setting::DataFolder => "Data folder",
            Setting::KeepRaw => "Keep raw exports (45 MB a match)",
            Setting::ReplayFolder => "Replay folder",
            Setting::Vrfkit => "vrfkit",
            Setting::Watch => "Watch the replay folder",
        }
    }

    /// The value of an On/Off setting.
    fn switch(self, cfg: &Config) -> Option<bool> {
        match self {
            Setting::AutoUpdate => Some(cfg.auto_update != Some(false)),
            Setting::KeepRaw => Some(cfg.keep_raw == Some(true)),
            Setting::Watch => Some(cfg.watch != Some(false)),
            _ => None,
        }
    }
}

/// A list of values to pick one from.
struct Picker {
    setting: Setting,
    /// (value, name, about)
    options: Vec<(String, String, String)>,
    sel: usize,
}

/// A path being typed in place of a value.
struct Field {
    setting: Setting,
    buf: Vec<char>,
    cur: usize,
}

pub enum SettingsAction {
    None,
    Close,
    /// `config.toml` changed: refresh what depends on it.
    Changed,
}

pub struct SettingsPanel {
    data_dir: PathBuf,
    update_note: Option<String>,
    status: StatusGroups,
    /// (subject, matches, agents) in 2+ matches, and the number of matches.
    accounts: Vec<(String, usize, Vec<String>)>,
    total: usize,
    tab: usize,
    sel: usize,
    query: String,
    searching: bool,
    picker: Option<Picker>,
    field: Option<Field>,
    /// Why the last change was not saved.
    error: Option<String>,
}

impl SettingsPanel {
    pub fn new(data_dir: &Path, update_note: Option<String>) -> Self {
        let mut panel = SettingsPanel {
            data_dir: data_dir.to_path_buf(),
            update_note,
            status: vec![],
            accounts: vec![],
            total: 0,
            tab: 0,
            sel: 0,
            query: String::new(),
            searching: false,
            picker: None,
            field: None,
            error: None,
        };
        panel.reload();
        panel
    }

    fn reload(&mut self) {
        self.status = status_groups(&self.data_dir, self.update_note.as_deref());
        let db = paths::db_path(&self.data_dir);
        let store = db.is_file().then(|| Store::open(&db).ok()).flatten();
        self.accounts = store
            .as_ref()
            .and_then(|s| s.accounts(2).ok())
            .unwrap_or_default()
            .into_iter()
            .take(ACCOUNTS)
            .collect();
        self.total = store.and_then(|s| s.match_count().ok()).unwrap_or(0);
    }

    fn config(&self) -> Config {
        Config::load(&self.data_dir).unwrap_or_default()
    }

    /// Change `config.toml`, keeping the error when it fails.
    fn save(&mut self, change: impl FnOnce(&mut Config)) -> SettingsAction {
        let mut cfg = match Config::load(&self.data_dir) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return SettingsAction::None;
            }
        };
        change(&mut cfg);
        match cfg.save(&self.data_dir) {
            Ok(()) => {
                self.error = None;
                self.reload();
                SettingsAction::Changed
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                SettingsAction::None
            }
        }
    }

    fn shown(&self) -> Vec<Setting> {
        let q = self.query.to_lowercase();
        SETTINGS
            .into_iter()
            .filter(|s| q.is_empty() || s.label().to_lowercase().contains(&q))
            .collect()
    }

    fn about_account(&self, subject: &str) -> String {
        match self.accounts.iter().find(|a| a.0 == subject) {
            Some((_, n, _)) if *n == self.total => format!("in all {n} matches"),
            Some((_, n, _)) => format!("in {n} matches"),
            None => "set by hand".to_owned(),
        }
    }

    /// Enter or Space on a setting.
    fn act(&mut self, setting: Setting) -> SettingsAction {
        let cfg = self.config();
        if let Some(on) = setting.switch(&cfg) {
            return self.save(|c| match setting {
                Setting::AutoUpdate => c.auto_update = Some(!on),
                Setting::KeepRaw => c.keep_raw = Some(!on),
                _ => c.watch = Some(!on),
            });
        }
        match setting {
            Setting::ReplayFolder => {
                let text = cfg.demos_dir().map(|d| d.display().to_string()).unwrap_or_default();
                self.field = Some(Field {
                    setting,
                    cur: text.chars().count(),
                    buf: text.chars().collect(),
                });
            }
            Setting::Vrfkit => {
                let own = cfg.vrfkit.is_some() && !installed_path(cfg.vrfkit.as_deref());
                self.picker = Some(Picker {
                    setting,
                    options: vec![
                        (
                            String::new(),
                            "Installed release".into(),
                            format!("{}, kept up to date by valreview", PINNED.tag),
                        ),
                        (
                            "own".into(),
                            "Your own copy…".into(),
                            "type the path to a vrfkit.exe".into(),
                        ),
                    ],
                    sel: usize::from(own),
                });
            }
            Setting::Account => {
                let options: Vec<_> = self
                    .accounts
                    .iter()
                    .map(|(s, _, agents)| {
                        let played = agents.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                        (s.clone(), s.clone(), format!("{} · {played}", self.about_account(s)))
                    })
                    .collect();
                let sel = options.iter().position(|o| Some(&o.0) == cfg.me.as_ref()).unwrap_or(0);
                self.picker = Some(Picker { setting, options, sel });
            }
            _ => {}
        }
        SettingsAction::None
    }

    pub fn key(&mut self, key: KeyEvent) -> SettingsAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let close = key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c'));
        if let Some(f) = &mut self.field {
            match key.code {
                KeyCode::Enter => {
                    let text: String = f.buf.iter().collect::<String>().trim().to_owned();
                    let setting = f.setting;
                    self.field = None;
                    if text.is_empty() {
                        return SettingsAction::None;
                    }
                    let path = std::path::absolute(&text).unwrap_or_else(|_| PathBuf::from(&text));
                    if setting == Setting::Vrfkit {
                        if !path.is_file() {
                            self.error = Some(format!("There's no file at {}", path.display()));
                            return SettingsAction::None;
                        }
                        return self.save(|c| c.vrfkit = Some(path));
                    }
                    return self.save(|c| c.demos_dir = Some(path));
                }
                _ if close => self.field = None,
                KeyCode::Left => f.cur = f.cur.saturating_sub(1),
                KeyCode::Right => f.cur = (f.cur + 1).min(f.buf.len()),
                KeyCode::Home => f.cur = 0,
                KeyCode::End => f.cur = f.buf.len(),
                KeyCode::Backspace if f.cur > 0 => {
                    f.cur -= 1;
                    f.buf.remove(f.cur);
                }
                KeyCode::Delete if f.cur < f.buf.len() => {
                    f.buf.remove(f.cur);
                }
                KeyCode::Char(c) if !ctrl => {
                    f.buf.insert(f.cur, c);
                    f.cur += 1;
                }
                _ => {}
            }
            return SettingsAction::None;
        }
        if let Some(p) = &mut self.picker {
            match key.code {
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Down => p.sel = (p.sel + 1).min(p.options.len().saturating_sub(1)),
                KeyCode::Left => self.picker = None,
                _ if close => self.picker = None,
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let p = self.picker.take().expect("checked");
                    let Some((value, _, _)) = p.options.get(p.sel).cloned() else {
                        return SettingsAction::None;
                    };
                    return match p.setting {
                        Setting::Account => self.save(|c| c.me = Some(value)),
                        _ if value == "own" => {
                            let own = self
                                .config()
                                .vrfkit
                                .filter(|v| !installed_path(Some(v)))
                                .map(|v| v.display().to_string())
                                .unwrap_or_default();
                            self.field = Some(Field {
                                setting: Setting::Vrfkit,
                                cur: own.chars().count(),
                                buf: own.chars().collect(),
                            });
                            SettingsAction::None
                        }
                        _ => self.save(|c| c.vrfkit = None),
                    };
                }
                _ => {}
            }
            return SettingsAction::None;
        }
        let shown = self.shown();
        if self.tab == 1 && self.searching {
            match key.code {
                _ if close => {
                    if self.query.is_empty() {
                        self.searching = false;
                    }
                    self.query.clear();
                    self.sel = 0;
                }
                KeyCode::Backspace => {
                    if self.query.pop().is_none() {
                        self.searching = false;
                    }
                    self.sel = 0;
                }
                KeyCode::Up => self.sel = self.sel.saturating_sub(1),
                KeyCode::Down => self.sel = (self.sel + 1).min(shown.len().saturating_sub(1)),
                KeyCode::Enter => {
                    if let Some(s) = shown.get(self.sel).copied() {
                        return self.act(s);
                    }
                }
                KeyCode::Char(c) if !ctrl => {
                    self.query.push(c);
                    self.sel = 0;
                }
                _ => {}
            }
            return SettingsAction::None;
        }
        match key.code {
            _ if close => return SettingsAction::Close,
            KeyCode::Char('q') => return SettingsAction::Close,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.tab = 1 - self.tab;
                self.error = None;
            }
            _ if self.tab != 1 => {}
            KeyCode::Char('/') => {
                self.searching = true;
                self.sel = 0;
            }
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(shown.len().saturating_sub(1)),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = shown.len().saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(s) = shown.get(self.sel).copied() {
                    return self.act(s);
                }
            }
            _ => {}
        }
        SettingsAction::None
    }

    /// The panel for a `w`-column screen: as tall as its content, at most `h` rows.
    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let mut top: Vec<Row> = vec![vec![(Style::fg(ACCENT), "─".repeat(w.saturating_sub(1)))], vec![]];
        let mut tabs: Row = vec![(Style::fg(ACCENT).bold(), "  Settings".into())];
        for (i, name) in TABS.iter().enumerate() {
            tabs.push((Style::default(), "  ".into()));
            let style = if i == self.tab {
                Style {
                    fg: Some(Color::Black),
                    bg: Some(BRIGHT),
                    bold: false,
                }
            } else {
                Style::default()
            };
            tabs.push((style, format!(" {name} ")));
        }
        top.push(tabs);
        top.push(vec![]);

        let mut status: Vec<Row> = Vec::new();
        for (i, group) in self.status.iter().enumerate() {
            if i > 0 {
                status.push(vec![]);
            }
            for (label, value) in group {
                let mut row: Row = vec![(Style::default(), format!("  {:<17}", format!("{label}:")))];
                row.extend(value.iter().map(|(k, t)| (kind_style(*k), t.clone())));
                status.push(row);
            }
        }
        let config = self.config_rows(w);
        let hint = if self.tab == 0 {
            "←→ tabs · Esc closes"
        } else if self.field.is_some() {
            "Type the path · Enter saves · Esc cancels"
        } else if self.picker.is_some() {
            "↑↓ choose · Enter picks · Esc goes back"
        } else if self.searching {
            "Type to filter · ↑↓ move · Enter changes it · Esc clears the search"
        } else {
            "Enter or Space changes it · / searches · ←→ tabs · Esc closes"
        };
        let bottom: Vec<Row> = vec![vec![], vec![(Style::fg(GREY), format!("  {hint}"))]];
        // As tall as the taller tab can get, so switching tabs keeps the height.
        let needed = status.len().max(4 + SETTINGS.len() + 2 + self.accounts.len().max(2));
        let body_h = needed.min(h.saturating_sub(top.len() + bottom.len())).max(1);
        let mut body = if self.tab == 0 { status } else { config };
        body.resize(body_h, vec![]);
        let mut rows = top;
        rows.extend(body);
        rows.extend(bottom);
        rows.truncate(h);
        rows
    }

    fn config_rows(&mut self, w: usize) -> Vec<Row> {
        let cfg = self.config();
        let mut rows: Vec<Row> = Vec::new();
        // The search box.
        let box_w = w.saturating_sub(6).max(20);
        let edge = Style::fg(if self.searching { ACCENT } else { RULE });
        let mut inside: Row = vec![(Style::fg(GREY), " ⌕ ".into())];
        if self.searching {
            let n = self.query.chars().count();
            inside.extend(field(&self.query.chars().collect::<Vec<_>>(), n, Style::default()));
        } else {
            inside.push((Style::fg(GREY), "Search settings…".into()));
        }
        let pad = box_w.saturating_sub(crate::shell::row_width(&inside));
        rows.push(vec![
            (Style::default(), "  ".into()),
            (edge, format!("╭{}╮", "─".repeat(box_w))),
        ]);
        let mut middle: Row = vec![(Style::default(), "  ".into()), (edge, "│".into())];
        middle.extend(inside);
        middle.push((Style::default(), " ".repeat(pad)));
        middle.push((edge, "│".into()));
        rows.push(middle);
        rows.push(vec![
            (Style::default(), "  ".into()),
            (edge, format!("╰{}╯", "─".repeat(box_w))),
        ]);
        rows.push(vec![]);

        let shown = self.shown();
        self.sel = self.sel.min(shown.len().saturating_sub(1));
        if shown.is_empty() {
            rows.push(vec![(
                Style::fg(GREY),
                format!("    No settings match \"{}\"", self.query),
            )]);
        }
        let query = self.query.to_lowercase();
        for (i, setting) in shown.iter().copied().enumerate() {
            let on = i == self.sel;
            let fixed = setting == Setting::DataFolder;
            let base = if fixed {
                Style::fg(GREY)
            } else if on {
                Style::fg(ACCENT)
            } else {
                Style::default()
            };
            let mut row: Row = vec![(
                if on { Style::fg(ACCENT) } else { Style::default() },
                if on { "  ❯ " } else { "    " }.into(),
            )];
            let label = format!("{:<LABEL_W$}", setting.label());
            if query.is_empty() {
                row.push((base, label));
            } else {
                row.extend(highlight(&label, &query, false, base));
            }
            match (&self.field, setting.switch(&cfg)) {
                (Some(f), _) if on && f.setting == setting => row.extend(field(&f.buf, f.cur, Style::default())),
                (_, Some(v)) => {
                    let style = Style::fg(if v { GREEN } else { RED });
                    row.push((
                        if on { style.bold() } else { style },
                        if v { "On" } else { "Off" }.into(),
                    ));
                }
                _ => {
                    let (style, value) = self.value(setting, &cfg);
                    row.push((
                        if fixed {
                            Style::fg(GREY)
                        } else if on {
                            base
                        } else {
                            style
                        },
                        value,
                    ));
                    if !fixed {
                        row.push((Style::fg(GREY), " ›".into()));
                    }
                }
            }
            rows.push(row);
            if let Some(p) = self.picker.as_ref().filter(|p| p.setting == setting) {
                if p.options.is_empty() {
                    rows.push(vec![(
                        Style::fg(GREY),
                        format!("{}No account is in 2 or more matches yet", " ".repeat(LABEL_W + 4)),
                    )]);
                }
                for (j, (_, name, about)) in p.options.iter().enumerate() {
                    let style = if j == p.sel {
                        Style::fg(ACCENT)
                    } else {
                        Style::default()
                    };
                    rows.push(vec![
                        (Style::default(), " ".repeat(LABEL_W + 4)),
                        (style, format!("{}{name}", if j == p.sel { "❯ " } else { "  " })),
                        (Style::fg(GREY), format!("  {about}")),
                    ]);
                }
            }
        }
        if let Some(e) = &self.error {
            rows.push(vec![]);
            rows.push(vec![(Style::fg(RED), format!("    {e}"))]);
        }
        rows
    }

    /// A setting's value as text, for those that are not On/Off.
    fn value(&self, setting: Setting, cfg: &Config) -> (Style, String) {
        match setting {
            Setting::Account => match &cfg.me {
                Some(me) => (
                    Style::default(),
                    format!("{} ({})", &me[..me.len().min(8)], self.about_account(me)),
                ),
                None => (Style::fg(crate::shell::YELLOW), "Not set".into()),
            },
            Setting::DataFolder => (Style::fg(GREY), self.data_dir.display().to_string()),
            Setting::ReplayFolder => match cfg.demos_dir() {
                Some(d) => (Style::default(), d.display().to_string()),
                None => (Style::fg(crate::shell::YELLOW), "Not set".into()),
            },
            Setting::Vrfkit => match &cfg.vrfkit {
                Some(p) if !installed_path(Some(p)) => (Style::default(), p.display().to_string()),
                _ => (Style::default(), format!("Installed release ({})", PINNED.tag)),
            },
            _ => (Style::default(), String::new()),
        }
    }
}

const RULE: Color = Color::Rgb { r: 110, g: 110, b: 110 };

/// A one-line text field: the text with a block cursor at `cur`.
fn field(buf: &[char], cur: usize, base: Style) -> Row {
    let mut row: Row = Vec::new();
    if cur > 0 {
        row.push((base, buf[..cur].iter().collect()));
    }
    row.push((
        Style {
            fg: Some(Color::Black),
            bg: Some(BRIGHT),
            bold: false,
        },
        buf.get(cur).map_or(" ".into(), |c| c.to_string()),
    ));
    if cur + 1 < buf.len() {
        row.push((base, buf[cur + 1..].iter().collect()));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_in_alphabetical_order() {
        let labels: Vec<String> = SETTINGS.iter().map(|s| s.label().to_lowercase()).collect();
        let mut sorted = labels.clone();
        sorted.sort();
        assert_eq!(labels, sorted);
    }

    #[test]
    fn field_puts_the_cursor_on_a_character() {
        let text = |r: &Row| r.iter().map(|(_, s)| s.as_str()).collect::<String>();
        let buf: Vec<char> = "C:\\x".chars().collect();
        assert_eq!(text(&field(&buf, 4, Style::default())), "C:\\x ");
        assert_eq!(text(&field(&buf, 0, Style::default())), "C:\\x");
        assert_eq!(field(&buf, 1, Style::default())[1].1, ":");
    }
}
