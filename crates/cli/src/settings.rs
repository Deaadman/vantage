//! `/settings`: what Vantage runs with (Status) and what you can change
//! (Config), drawn in place of the input box until Esc. Outside the shell,
//! `vantage settings` prints the Status tab.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use vantage_core::clips::{self, ShotStyle};
use vantage_core::config::Config;
use vantage_core::store::Store;
use vantage_core::{ai, habits, paths, ranks};

use crate::out::{self, Kind, Line};
use crate::panel::{pad, rank_color};
use crate::review_view::GREEN;
use crate::shell::{ACCENT, BRIGHT, GREY, RED, Row, Style, YELLOW, kind_style};

const TABS: [&str; 2] = ["Status", "Config"];
const LABEL_W: usize = 26;
/// At most this many accounts to pick from.
const ACCOUNTS: usize = 8;
/// Picker rows shown at once.
const PICK_ROWS: usize = 8;

/// The Status tab: labelled values in groups.
pub type StatusGroups = Vec<Vec<(&'static str, Line)>>;

fn piece(kind: Kind, text: impl Into<String>) -> (Kind, String) {
    (kind, text.into())
}

/// Everything the Status tab shows: values only, no hints.
pub fn status_groups(data_dir: &Path) -> StatusGroups {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let rank = match &cfg.rank {
        Some(r) => vec![piece(Kind::Paint(rank_color(Some(r)), false), r.clone())],
        None => vec![piece(Kind::Warn, "Unranked")],
    };
    let account = match &cfg.me {
        Some(me) => vec![piece(Kind::Out, me.clone())],
        None => vec![piece(Kind::Warn, "Not found yet")],
    };
    let ai = match cfg.ai.as_deref().and_then(ai::provider) {
        Some(p) => vec![piece(
            Kind::Out,
            format!(
                "{} · {}",
                p.name,
                ai::model_name(ai::chosen_model(p, cfg.ai_model.as_deref()))
            ),
        )],
        None => vec![piece(Kind::Dim, "Off")],
    };
    let db = paths::db_path(data_dir);
    let matches = match db.is_file().then(|| Store::open(&db).and_then(|s| s.counts())) {
        Some(Ok(c)) if c.matches > 0 => vec![piece(
            Kind::Out,
            if c.logged > 0 {
                format!("{} stored, {} couldn't be read", c.matches, c.logged)
            } else {
                format!("{} stored", c.matches)
            },
        )],
        Some(Err(e)) => vec![piece(Kind::Error, format!("Unreadable: {e:#}"))],
        _ => vec![piece(Kind::Dim, "None yet")],
    };
    let replays = match cfg.demos_dir() {
        Some(d) if d.is_dir() => vec![piece(Kind::Out, d.display().to_string())],
        Some(d) => vec![piece(Kind::Warn, format!("{} (not found)", d.display()))],
        None => vec![piece(Kind::Warn, "Not set")],
    };
    let clip_dir = match &cfg.clips_dir {
        Some(d) => vec![piece(Kind::Out, d.display().to_string())],
        None => vec![piece(Kind::Warn, "Not set")],
    };
    let shots = clips::shots_root().map_or_else(|| "?".into(), |p| p.display().to_string());
    vec![
        vec![
            ("Version", vec![piece(Kind::Out, crate::VERSION)]),
            ("Rank", rank),
            (
                "Focus",
                vec![piece(Kind::Out, habits::focus(cfg.focus.as_deref()).name)],
            ),
            ("Account", account),
            ("AI review", ai),
        ],
        vec![
            ("Matches", matches),
            ("Replays", replays),
            ("Clips", clip_dir),
            ("Screenshots", vec![piece(Kind::Out, shots)]),
            (
                "Logs",
                vec![piece(
                    Kind::Out,
                    crate::session_log::dir(data_dir).display().to_string(),
                )],
            ),
        ],
    ]
}

/// `vantage settings` outside the shell: the Status tab as text.
pub fn print_status(data_dir: &Path) {
    for (i, group) in status_groups(data_dir).into_iter().enumerate() {
        if i > 0 {
            outln!();
        }
        for (label, value) in group {
            let mut row = vec![piece(Kind::Out, format!("{:<14}", format!("{label}:")))];
            row.extend(value);
            out::row(row);
        }
    }
}

/// One line of the Config tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
    Rank,
    Focus,
    Account,
    Ai,
    AiModel,
    Shots,
    ReplayFolder,
    ClipFolder,
    Watch,
    AutoUpdate,
}

/// The Config tab grouped like Status: choices, then folders, then on/off switches.
const GROUPS: [&[Setting]; 3] = [
    &[
        Setting::Rank,
        Setting::Focus,
        Setting::Account,
        Setting::Ai,
        Setting::AiModel,
        Setting::Shots,
    ],
    &[Setting::ReplayFolder, Setting::ClipFolder],
    &[Setting::Watch, Setting::AutoUpdate],
];
/// Every setting, and the blank rows between the groups.
const SETTING_ROWS: usize = 12;

/// The settings shown, group by group: the AI model only once an AI is picked.
fn shown(cfg: &Config) -> Vec<Vec<Setting>> {
    GROUPS
        .iter()
        .map(|g| {
            g.iter()
                .copied()
                .filter(|s| *s != Setting::AiModel || cfg.ai.as_deref().and_then(ai::provider).is_some())
                .collect()
        })
        .collect()
}

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Setting::Ai => "AI review",
            Setting::AiModel => "AI model",
            Setting::AutoUpdate => "Automatic updates",
            Setting::ClipFolder => "Clip folder",
            Setting::Focus => "Focus habit",
            Setting::Rank => "Rank",
            Setting::ReplayFolder => "Replay folder",
            Setting::Shots => "Screenshots per death",
            Setting::Watch => "Watch the replay folder",
            Setting::Account => "Your account",
        }
    }

    /// The value of an On/Off setting.
    fn switch(self, cfg: &Config) -> Option<bool> {
        match self {
            Setting::AutoUpdate => Some(cfg.auto_update != Some(false)),
            Setting::Watch => Some(cfg.watch != Some(false)),
            _ => None,
        }
    }
}

/// One value to pick: (value, name, about).
type Choice = (String, String, String);

/// The AI's models, and why the usual ones are offered instead when the list can't be fetched.
type Models = (Vec<Choice>, Option<String>);

/// A list of values to pick one from.
struct Picker {
    setting: Setting,
    options: Vec<(String, String, String)>,
    sel: usize,
    /// The AI's models, while they're being fetched: who is asked, and the answer.
    loading: Option<(&'static str, Receiver<Models>)>,
    /// Why the list couldn't be fetched; the models offered are a fallback.
    note: Option<String>,
}

/// Text being typed in place of a value: a path, or a masked API key.
struct Field {
    setting: Setting,
    buf: Vec<char>,
    cur: usize,
    /// The AI provider the key is for; the text shows as dots.
    provider: Option<&'static ai::Provider>,
}

/// "Another model": type its id.
const OTHER_MODEL: &str = "";

pub enum SettingsAction {
    None,
    Close,
    /// `config.toml` changed: refresh what depends on it.
    Changed,
}

pub struct SettingsPanel {
    data_dir: PathBuf,
    status: StatusGroups,
    /// (subject, matches, agents) in 2+ matches, and the number of matches.
    accounts: Vec<(String, usize, Vec<String>)>,
    total: usize,
    tab: usize,
    sel: usize,
    picker: Option<Picker>,
    field: Option<Field>,
    /// Why the last change was not saved.
    error: Option<String>,
}

impl SettingsPanel {
    pub fn new(data_dir: &Path) -> Self {
        let mut panel = SettingsPanel {
            data_dir: data_dir.to_path_buf(),
            status: vec![],
            accounts: vec![],
            total: 0,
            tab: 0,
            sel: 0,
            picker: None,
            field: None,
            error: None,
        };
        panel.reload();
        panel
    }

    fn reload(&mut self) {
        self.status = status_groups(&self.data_dir);
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

    fn about_account(&self, subject: &str) -> String {
        match self.accounts.iter().find(|a| a.0 == subject) {
            Some((_, n, _)) if *n == self.total => format!("in all {n} matches"),
            Some((_, n, _)) => format!("in {n} matches"),
            None => "set by hand".to_owned(),
        }
    }

    fn options(&self, setting: Setting) -> Vec<(String, String, String)> {
        let o = |v: &str, n: &str, a: &str| (v.to_owned(), n.to_owned(), a.to_owned());
        match setting {
            Setting::Ai => {
                let mut v = vec![o("off", "Off", "the review works without it")];
                v.extend(ai::PROVIDERS.iter().map(|p| {
                    let about = match p.key {
                        Some(k) => format!("with your {k}"),
                        None => "runs a model on this PC, no key needed".into(),
                    };
                    (p.id.to_owned(), p.name.to_owned(), about)
                }));
                v
            }
            Setting::ClipFolder => crate::clip_folders()
                .into_iter()
                .map(|f| match f.path {
                    Some(p) => (p.display().to_string(), f.name, p.display().to_string()),
                    None => (String::new(), f.name, "type the path".into()),
                })
                .collect(),
            Setting::Focus => habits::HABITS
                .iter()
                .map(|h| o(h.slug, h.name, &h.group.to_lowercase()))
                .collect(),
            Setting::Rank => ranks::all()
                .into_iter()
                .map(|r| (r.clone(), r, String::new()))
                .collect(),
            Setting::Shots => ShotStyle::ALL.iter().map(|s| o(s.key(), s.name(), s.about())).collect(),
            Setting::Account => self
                .accounts
                .iter()
                .map(|(s, _, agents)| {
                    let played = agents.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                    (s.clone(), s.clone(), format!("{} · {played}", self.about_account(s)))
                })
                .collect(),
            _ => vec![],
        }
    }

    fn current(&self, setting: Setting, cfg: &Config) -> Option<String> {
        match setting {
            Setting::Ai => Some(cfg.ai.clone().unwrap_or_else(|| "off".into())),
            Setting::AiModel => cfg
                .ai
                .as_deref()
                .and_then(ai::provider)
                .map(|p| ai::chosen_model(p, cfg.ai_model.as_deref()).to_owned()),
            Setting::ClipFolder => cfg.clips_dir.as_ref().map(|p| p.display().to_string()),
            Setting::Focus => Some(habits::focus(cfg.focus.as_deref()).slug.to_owned()),
            Setting::Rank => cfg.rank.clone(),
            Setting::Shots => Some(ShotStyle::parse(cfg.shots.as_deref()).key().to_owned()),
            Setting::Account => cfg.me.clone(),
            _ => None,
        }
    }

    fn edit(&mut self, setting: Setting, text: String, provider: Option<&'static ai::Provider>) {
        self.field = Some(Field {
            setting,
            cur: text.chars().count(),
            buf: text.chars().collect(),
            provider,
        });
    }

    /// Enter or Space on a setting.
    fn act(&mut self, setting: Setting) -> SettingsAction {
        let cfg = self.config();
        if let Some(on) = setting.switch(&cfg) {
            return self.save(|c| match setting {
                Setting::AutoUpdate => c.auto_update = Some(!on),
                _ => c.watch = Some(!on),
            });
        }
        if setting == Setting::ReplayFolder {
            let text = cfg.demos_dir().map(|d| d.display().to_string()).unwrap_or_default();
            self.edit(setting, text, None);
            return SettingsAction::None;
        }
        if setting == Setting::AiModel {
            return self.pick_model(&cfg);
        }
        let options = self.options(setting);
        let cur = self.current(setting, &cfg);
        let sel = options.iter().position(|o| Some(&o.0) == cur.as_ref()).unwrap_or(0);
        self.picker = Some(Picker {
            setting,
            options,
            sel,
            loading: None,
            note: None,
        });
        SettingsAction::None
    }

    /// The AI model picker: the provider's models, fetched in the background.
    fn pick_model(&mut self, cfg: &Config) -> SettingsAction {
        let Some(p) = cfg.ai.as_deref().and_then(ai::provider) else {
            return SettingsAction::None;
        };
        let (tx, rx) = mpsc::channel();
        let key = cfg.ai_key.clone();
        std::thread::spawn(move || {
            let (models, note) = match ai::models(p, key.as_deref()) {
                Ok(m) => (m, None),
                Err(e) => (ai::fallback_models(p), Some(format!("{e:#}"))),
            };
            let mut options: Vec<(String, String, String)> =
                models.into_iter().map(|m| (m.id, m.name, m.about)).collect();
            options.push((
                OTHER_MODEL.into(),
                "Another model".into(),
                if p.local {
                    "type a name to pull".into()
                } else {
                    "type its model ID".into()
                },
            ));
            let _ = tx.send((options, note));
        });
        self.picker = Some(Picker {
            setting: Setting::AiModel,
            options: vec![],
            sel: 0,
            loading: Some((p.name, rx)),
            note: None,
        });
        SettingsAction::None
    }

    /// True while a list is being fetched, so the shell keeps drawing.
    pub fn busy(&mut self) -> bool {
        let Some((_, rx)) = self.picker.as_ref().and_then(|p| p.loading.as_ref()) else {
            return false;
        };
        let got = match rx.try_recv() {
            Ok(options) => Some(options),
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let cur = self.current(Setting::AiModel, &self.config());
        let p = self.picker.as_mut().expect("checked");
        p.loading = None;
        if let Some((options, note)) = got {
            p.note = note;
            p.sel = options.iter().position(|o| Some(&o.0) == cur.as_ref()).unwrap_or(0);
            p.options = options;
        }
        true
    }

    /// Text pasted while a field is open goes into it.
    pub fn paste(&mut self, text: &str) {
        if let Some(f) = &mut self.field {
            for c in text.chars().filter(|c| !c.is_control()) {
                f.buf.insert(f.cur, c);
                f.cur += 1;
            }
        }
    }

    fn picked(&mut self, setting: Setting, value: String) -> SettingsAction {
        match setting {
            Setting::Ai => match value.as_str() {
                "off" => self.save(|c| {
                    c.ai = None;
                    c.ai_key = None;
                    c.ai_model = None;
                }),
                id => match ai::provider(id) {
                    Some(p) if p.local => self.save(|c| {
                        c.ai = Some(p.id.into());
                        c.ai_key = None;
                        c.ai_model = None;
                    }),
                    Some(p) => {
                        self.edit(setting, String::new(), Some(p));
                        SettingsAction::None
                    }
                    None => SettingsAction::None,
                },
            },
            Setting::ClipFolder if value.is_empty() => {
                let start = self
                    .config()
                    .clips_dir
                    .or_else(clips::videos_dir)
                    .map(|p| format!("{}\\", p.display()))
                    .unwrap_or_default();
                self.edit(setting, start, None);
                SettingsAction::None
            }
            Setting::ClipFolder => self.save(|c| c.clips_dir = Some(PathBuf::from(value))),
            Setting::AiModel if value == OTHER_MODEL => {
                self.edit(setting, String::new(), None);
                SettingsAction::None
            }
            Setting::AiModel => self.save(|c| c.ai_model = Some(value)),
            Setting::Focus => self.save(|c| c.focus = Some(value)),
            Setting::Rank => self.save(|c| c.rank = Some(value)),
            Setting::Shots => self.save(|c| c.shots = Some(value)),
            Setting::Account => self.save(|c| c.me = Some(value)),
            _ => SettingsAction::None,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> SettingsAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let close = key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c'));
        if let Some(f) = &mut self.field {
            match key.code {
                KeyCode::Enter => {
                    let text: String = f.buf.iter().collect::<String>().trim().to_owned();
                    let (setting, provider) = (f.setting, f.provider);
                    self.field = None;
                    if text.is_empty() {
                        return SettingsAction::None;
                    }
                    if let Some(p) = provider {
                        return self.save(|c| {
                            if c.ai.as_deref() != Some(p.id) {
                                c.ai_model = None;
                            }
                            c.ai = Some(p.id.into());
                            c.ai_key = Some(text);
                        });
                    }
                    if setting == Setting::AiModel {
                        return self.save(|c| c.ai_model = Some(text));
                    }
                    let path = std::path::absolute(&text).unwrap_or_else(|_| PathBuf::from(&text));
                    if setting == Setting::ClipFolder {
                        if !path.is_dir() {
                            self.error = Some(format!("There's no folder at {}", path.display()));
                            return SettingsAction::None;
                        }
                        return self.save(|c| c.clips_dir = Some(path));
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
                KeyCode::Char('v') if ctrl => {}
                KeyCode::Char(c) if !ctrl => {
                    f.buf.insert(f.cur, c);
                    f.cur += 1;
                }
                _ => {}
            }
            return SettingsAction::None;
        }
        if let Some(p) = &mut self.picker {
            if p.loading.is_some() {
                if close || key.code == KeyCode::Left {
                    self.picker = None;
                }
                return SettingsAction::None;
            }
            match key.code {
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Down => p.sel = (p.sel + 1).min(p.options.len().saturating_sub(1)),
                KeyCode::PageUp => p.sel = p.sel.saturating_sub(PICK_ROWS),
                KeyCode::PageDown => p.sel = (p.sel + PICK_ROWS).min(p.options.len().saturating_sub(1)),
                KeyCode::Left => self.picker = None,
                _ if close => self.picker = None,
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let p = self.picker.take().expect("checked");
                    let Some((value, _, _)) = p.options.get(p.sel).cloned() else {
                        return SettingsAction::None;
                    };
                    return self.picked(p.setting, value);
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
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel += 1,
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = usize::MAX,
            KeyCode::Enter | KeyCode::Char(' ') => {
                let all: Vec<Setting> = shown(&self.config()).concat();
                self.sel = self.sel.min(all.len() - 1);
                return self.act(all[self.sel]);
            }
            _ => {}
        }
        self.sel = self.sel.min(shown(&self.config()).concat().len() - 1);
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
                let mut row: Row = vec![(Style::default(), format!("  {}", pad(&format!("{label}:"), 14)))];
                row.extend(value.iter().map(|(k, t)| (kind_style(*k), t.clone())));
                status.push(row);
            }
        }
        let config = self.config_rows();
        let hint = if self.tab == 0 {
            "←→ tabs · Esc closes"
        } else if let Some(f) = &self.field {
            if f.provider.is_some() {
                "Paste the key · it stays on this computer · Enter saves · Esc cancels"
            } else if f.setting == Setting::AiModel {
                "Type the model's name · Enter saves · Esc cancels"
            } else {
                "Type the path · Enter saves · Esc cancels"
            }
        } else if self.picker.is_some() {
            "↑↓ choose · Enter picks · Esc goes back"
        } else {
            "Enter or Space changes it · ←→ tabs · Esc closes"
        };
        let bottom: Vec<Row> = vec![vec![], vec![(Style::fg(GREY), format!("  {hint}"))]];
        // As tall as the Config tab can get with a picker open, so switching tabs keeps the height.
        let needed = status.len().max(SETTING_ROWS + PICK_ROWS + 4);
        let body_h = needed.min(h.saturating_sub(top.len() + bottom.len())).max(1);
        let mut body = if self.tab == 0 { status } else { config };
        // Keep the open picker in view on a short screen.
        let skip = body.len().saturating_sub(body_h).min(self.sel);
        body.drain(..skip);
        body.resize(body_h, vec![]);
        let mut rows = top;
        rows.extend(body);
        rows.extend(bottom);
        rows.truncate(h);
        rows
    }

    fn config_rows(&mut self) -> Vec<Row> {
        let cfg = self.config();
        let mut rows: Vec<Row> = Vec::new();
        let mut i = 0;
        for (g, group) in shown(&cfg).into_iter().enumerate() {
            if g > 0 {
                rows.push(vec![]);
            }
            for setting in group {
                self.setting_rows(&mut rows, setting, i == self.sel, &cfg);
                i += 1;
            }
        }
        if let Some(e) = &self.error {
            rows.push(vec![]);
            rows.push(vec![(Style::fg(RED), format!("    {e}"))]);
        }
        rows
    }

    /// One setting's row, and its picker when open.
    fn setting_rows(&self, rows: &mut Vec<Row>, setting: Setting, on: bool, cfg: &Config) {
        {
            let base = if on { Style::fg(ACCENT) } else { Style::default() };
            let mut row: Row = vec![
                (base, if on { "  ❯ " } else { "    " }.into()),
                (base, pad(setting.label(), LABEL_W)),
            ];
            match (&self.field, setting.switch(cfg)) {
                (Some(f), _) if on && f.setting == setting => {
                    let shown: Vec<char> = if f.provider.is_some() {
                        vec!['•'; f.buf.len()]
                    } else {
                        f.buf.clone()
                    };
                    row.extend(field(&shown, f.cur, Style::default()));
                    if let Some(p) = f.provider.filter(|_| f.buf.is_empty()) {
                        row.push((Style::fg(GREY), format!("paste your {}", p.key.unwrap_or("key"))));
                    }
                }
                (_, Some(v)) => {
                    let style = Style::fg(if v { GREEN } else { RED });
                    row.push((
                        if on { style.bold() } else { style },
                        if v { "On" } else { "Off" }.into(),
                    ));
                }
                _ => {
                    let (style, value) = value(setting, cfg);
                    row.push((if on && style == Style::default() { base } else { style }, value));
                    row.push((Style::fg(GREY), " ›".into()));
                }
            }
            rows.push(row);
            if let Some(p) = self.picker.as_ref().filter(|p| p.setting == setting) {
                let indent = " ".repeat(LABEL_W + 4);
                if let Some((who, _)) = &p.loading {
                    rows.push(vec![(Style::fg(GREY), format!("{indent}Asking {who} for its models…"))]);
                    return;
                }
                if let Some(n) = &p.note {
                    let why: String = if n.chars().count() > 76 {
                        n.chars().take(75).chain(std::iter::once('…')).collect()
                    } else {
                        n.clone()
                    };
                    rows.push(vec![(
                        Style::fg(YELLOW),
                        format!("{indent}Couldn't get the list, so these are the usual ones:"),
                    )]);
                    rows.push(vec![(Style::fg(GREY), format!("{indent}{why}"))]);
                }
                if p.options.is_empty() {
                    rows.push(vec![(
                        Style::fg(GREY),
                        format!(
                            "{indent}{}",
                            if setting == Setting::Account {
                                "No account is in 2 or more matches yet"
                            } else {
                                "Nothing to pick from"
                            }
                        ),
                    )]);
                }
                let name_w = p
                    .options
                    .iter()
                    .map(|o| o.1.chars().count() + 2)
                    .max()
                    .unwrap_or(0)
                    .max(LABEL_W);
                let first = p.sel.saturating_sub(4).min(p.options.len().saturating_sub(PICK_ROWS));
                // A row says how many are above, so one fewer option shows.
                let shown = if first > 0 { PICK_ROWS - 1 } else { PICK_ROWS };
                let first = if first > 0 { (first + 1).min(p.sel) } else { 0 };
                if first > 0 {
                    rows.push(vec![(Style::fg(GREY), format!("{indent}↑ {first} more"))]);
                }
                for (j, (_, name, about)) in p.options.iter().enumerate().skip(first).take(shown) {
                    let pon = j == p.sel;
                    let name_style = if setting == Setting::Rank {
                        let s = Style::fg(rank_color(Some(name)));
                        if pon { s.bold() } else { s }
                    } else if pon {
                        Style::fg(ACCENT)
                    } else {
                        Style::default()
                    };
                    rows.push(vec![
                        (Style::default(), indent.clone()),
                        (
                            if pon { Style::fg(ACCENT) } else { Style::default() },
                            if pon { "❯ " } else { "  " }.into(),
                        ),
                        (name_style, pad(name, name_w)),
                        (Style::fg(GREY), about.clone()),
                    ]);
                }
                let below = p.options.len().saturating_sub(first + shown);
                if below > 0 {
                    rows.push(vec![(Style::fg(GREY), format!("{indent}↓ {below} more"))]);
                }
            }
        }
    }
}

/// A setting's value as text, for those that are not On/Off.
fn value(setting: Setting, cfg: &Config) -> (Style, String) {
    let unset = |t: &str| (Style::fg(YELLOW), t.to_owned());
    match setting {
        Setting::Ai => match cfg.ai.as_deref().and_then(ai::provider) {
            None => (Style::default(), "Off".into()),
            Some(p) if p.local => (Style::default(), format!("{} · on this PC", p.name)),
            Some(p) => {
                let key = cfg.ai_key.as_deref().unwrap_or("");
                let tail: String = key
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                (Style::default(), format!("{} · key ••••{tail}", p.name))
            }
        },
        Setting::ClipFolder => match &cfg.clips_dir {
            Some(d) => (Style::default(), d.display().to_string()),
            None => unset("Not set"),
        },
        Setting::AiModel => match cfg.ai.as_deref().and_then(ai::provider) {
            Some(p) => (
                Style::default(),
                ai::model_name(ai::chosen_model(p, cfg.ai_model.as_deref())),
            ),
            None => (Style::default(), String::new()),
        },
        Setting::Focus => (Style::default(), habits::focus(cfg.focus.as_deref()).name.into()),
        Setting::Rank => match &cfg.rank {
            Some(r) => (Style::fg(rank_color(Some(r))), r.clone()),
            None => unset("Not set"),
        },
        Setting::ReplayFolder => match cfg.demos_dir() {
            Some(d) => (Style::default(), d.display().to_string()),
            None => unset("Not set"),
        },
        Setting::Shots => (Style::default(), ShotStyle::parse(cfg.shots.as_deref()).name().into()),
        Setting::Account => match &cfg.me {
            Some(me) => (Style::default(), me.clone()),
            None => unset("Not found yet"),
        },
        _ => (Style::default(), String::new()),
    }
}

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
    fn settings_are_grouped_and_the_ai_model_shows_once_an_ai_is_picked() {
        let labels = |cfg: &Config| -> Vec<Vec<&str>> {
            shown(cfg)
                .iter()
                .map(|g| g.iter().map(|s| s.label()).collect())
                .collect()
        };
        let mut cfg = Config::default();
        assert_eq!(
            labels(&cfg),
            [
                vec![
                    "Rank",
                    "Focus habit",
                    "Your account",
                    "AI review",
                    "Screenshots per death"
                ],
                vec!["Replay folder", "Clip folder"],
                vec!["Watch the replay folder", "Automatic updates"],
            ]
        );
        cfg.ai = Some("anthropic".into());
        assert_eq!(labels(&cfg)[0][4], "AI model");
        assert_eq!(
            SETTING_ROWS,
            GROUPS.iter().map(|g| g.len()).sum::<usize>() + GROUPS.len() - 1
        );
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
