//! `/settings`: what Vantage runs with (Status) and what you can change
//! (Config), in Abalone's settings panel. Outside the shell,
//! `vantage settings` prints the Status tab.

use std::path::{Path, PathBuf};

use abalone::{Choice, Field, Loaded, Outcome, Picker, SettingRow, SettingsModel, StatusGroups, Value};
use vantage_core::clips::{self, ShotStyle};
use vantage_core::config::Config;
use vantage_core::store::Store;
use vantage_core::{ai, habits, paths, ranks};

use crate::out::{self, Kind};
use crate::panel::rank_color;
use crate::shell::Style;

/// At most this many accounts to pick from.
const ACCOUNTS: usize = 8;

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
    let label = |s: &str| s.to_owned();
    vec![
        vec![
            (label("Version"), vec![piece(Kind::Out, format!("v{}", crate::VERSION))]),
            (label("Rank"), rank),
            (
                label("Focus"),
                vec![piece(Kind::Out, habits::focus(cfg.focus.as_deref()).name)],
            ),
            (label("Account"), account),
            (label("AI review"), ai),
        ],
        vec![
            (label("Matches"), matches),
            (label("Replays"), replays),
            (label("Clips"), clip_dir),
            (label("Screenshots"), vec![piece(Kind::Out, shots)]),
            (
                label("Logs"),
                vec![piece(
                    Kind::Out,
                    vantage_core::paths::logs_dir(data_dir).display().to_string(),
                )],
            ),
        ],
    ]
}

/// `vantage settings` outside the shell: the Status tab as text.
pub fn print_status(data_dir: &Path) {
    abalone::settings::status_lines(&status_groups(data_dir))
        .into_iter()
        .for_each(out::row);
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
    const ALL: [Setting; 10] = [
        Setting::Rank,
        Setting::Focus,
        Setting::Account,
        Setting::Ai,
        Setting::AiModel,
        Setting::Shots,
        Setting::ReplayFolder,
        Setting::ClipFolder,
        Setting::Watch,
        Setting::AutoUpdate,
    ];

    /// The row's id in the panel.
    fn id(self) -> &'static str {
        match self {
            Setting::Ai => "ai",
            Setting::AiModel => "ai_model",
            Setting::AutoUpdate => "auto_update",
            Setting::ClipFolder => "clip_folder",
            Setting::Focus => "focus",
            Setting::Rank => "rank",
            Setting::ReplayFolder => "replay_folder",
            Setting::Shots => "shots",
            Setting::Watch => "watch",
            Setting::Account => "account",
        }
    }

    fn from_id(id: &str) -> Option<Setting> {
        Setting::ALL.into_iter().find(|s| s.id() == id)
    }

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

/// "Another model": type its id.
const OTHER_MODEL: &str = "";
/// A field for an AI provider's key: `ai_key:<provider id>`.
const AI_KEY: &str = "ai_key:";
const PATH_HINT: &str = "Type the path · Enter saves · Esc cancels";

/// Vantage's settings, kept in `config.toml`.
pub struct VantageSettings {
    data_dir: PathBuf,
    /// (subject, matches, agents) in 2+ matches, and the number of matches.
    accounts: Vec<(String, usize, Vec<String>)>,
    total: usize,
}

impl VantageSettings {
    pub fn new(data_dir: &Path) -> Self {
        let mut s = VantageSettings {
            data_dir: data_dir.to_path_buf(),
            accounts: vec![],
            total: 0,
        };
        s.reload();
        s
    }

    fn reload(&mut self) {
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

    /// Change `config.toml`, saying why when it fails.
    fn save(&mut self, change: impl FnOnce(&mut Config)) -> Outcome {
        let mut cfg = match Config::load(&self.data_dir) {
            Ok(c) => c,
            Err(e) => return Outcome::Error(format!("{e:#}")),
        };
        change(&mut cfg);
        match cfg.save(&self.data_dir) {
            Ok(()) => {
                self.reload();
                Outcome::Changed
            }
            Err(e) => Outcome::Error(format!("{e:#}")),
        }
    }

    fn about_account(&self, subject: &str) -> String {
        match self.accounts.iter().find(|a| a.0 == subject) {
            Some((_, n, _)) if *n == self.total => format!("in all {n} matches"),
            Some((_, n, _)) => format!("in {n} matches"),
            None => "set by hand".to_owned(),
        }
    }

    fn options(&self, setting: Setting) -> Vec<Choice> {
        match setting {
            Setting::Ai => {
                let mut v = vec![Choice::new("off", "Off", "the review works without it")];
                v.extend(ai::PROVIDERS.iter().map(|p| {
                    let about = match p.key {
                        Some(k) => format!("with your {k}"),
                        None => "runs a model on this PC, no key needed".into(),
                    };
                    Choice::new(p.id, p.name, about)
                }));
                v
            }
            Setting::ClipFolder => crate::clip_folders()
                .into_iter()
                .map(|f| match f.path {
                    Some(p) => Choice::new(p.display().to_string(), f.name, p.display().to_string()),
                    None => Choice::new("", f.name, "type the path"),
                })
                .collect(),
            Setting::Focus => habits::HABITS
                .iter()
                .map(|h| Choice::new(h.slug, h.name, h.group.to_lowercase()))
                .collect(),
            Setting::Rank => ranks::all()
                .into_iter()
                .map(|r| Choice::new(r.clone(), r.clone(), "").styled(Style::fg(rank_color(Some(&r)))))
                .collect(),
            Setting::Shots => ShotStyle::ALL
                .iter()
                .map(|s| Choice::new(s.key(), s.name(), s.about()))
                .collect(),
            Setting::Account => self
                .accounts
                .iter()
                .map(|(s, _, agents)| {
                    let played = agents.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                    Choice::new(s.clone(), s.clone(), format!("{} · {played}", self.about_account(s)))
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

    /// The AI model picker: the provider's models, fetched in the background.
    fn pick_model(&self, cfg: &Config) -> Outcome {
        let Some(p) = cfg.ai.as_deref().and_then(ai::provider) else {
            return Outcome::None;
        };
        let key = cfg.ai_key.clone();
        let current = self.current(Setting::AiModel, cfg);
        Outcome::Pick(Picker::loading(
            Setting::AiModel.id(),
            format!("Asking {} for its models…", p.name),
            current,
            move || {
                let (models, note) = match ai::models(p, key.as_deref()) {
                    Ok(m) => (m, None),
                    Err(e) => (ai::fallback_models(p), Some(format!("{e:#}"))),
                };
                let mut options: Vec<Choice> = models.into_iter().map(|m| Choice::new(m.id, m.name, m.about)).collect();
                options.push(Choice::new(
                    OTHER_MODEL,
                    "Another model",
                    if p.local {
                        "type a name to pull"
                    } else {
                        "type its model ID"
                    },
                ));
                Loaded { options, note }
            },
        ))
    }
}

impl SettingsModel for VantageSettings {
    fn status(&mut self) -> StatusGroups {
        status_groups(&self.data_dir)
    }

    fn groups(&mut self) -> Vec<Vec<SettingRow>> {
        let cfg = self.config();
        shown(&cfg)
            .into_iter()
            .map(|g| {
                g.into_iter()
                    .map(|s| SettingRow::new(s.id(), s.label(), value(s, &cfg)))
                    .collect()
            })
            .collect()
    }

    fn max_rows(&mut self) -> usize {
        SETTING_ROWS
    }

    fn activate(&mut self, id: &str) -> Outcome {
        let Some(setting) = Setting::from_id(id) else {
            return Outcome::None;
        };
        let cfg = self.config();
        if let Some(on) = setting.switch(&cfg) {
            return self.save(|c| match setting {
                Setting::AutoUpdate => c.auto_update = Some(!on),
                _ => c.watch = Some(!on),
            });
        }
        if setting == Setting::ReplayFolder {
            let text = cfg.demos_dir().map(|d| d.display().to_string()).unwrap_or_default();
            return Outcome::Edit(Field::new(id, &text).hint(PATH_HINT));
        }
        if setting == Setting::AiModel {
            return self.pick_model(&cfg);
        }
        let picker = Picker::new(id, self.options(setting), self.current(setting, &cfg).as_deref());
        Outcome::Pick(if setting == Setting::Account {
            picker.empty("No account is in 2 or more matches yet")
        } else {
            picker
        })
    }

    fn picked(&mut self, id: &str, value: &str) -> Outcome {
        let value = value.to_owned();
        match Setting::from_id(id) {
            Some(Setting::Ai) => match value.as_str() {
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
                    Some(p) => Outcome::Edit(
                        Field::new(Setting::Ai.id(), "")
                            .id(format!("{AI_KEY}{}", p.id))
                            .secret()
                            .placeholder(format!("paste your {}", p.key.unwrap_or("key")))
                            .hint("Paste the key · it stays on this computer · Enter saves · Esc cancels"),
                    ),
                    None => Outcome::None,
                },
            },
            Some(Setting::ClipFolder) if value.is_empty() => {
                let start = self
                    .config()
                    .clips_dir
                    .or_else(clips::videos_dir)
                    .map(|p| format!("{}\\", p.display()))
                    .unwrap_or_default();
                Outcome::Edit(Field::new(id, &start).hint(PATH_HINT))
            }
            Some(Setting::ClipFolder) => self.save(|c| c.clips_dir = Some(PathBuf::from(value))),
            Some(Setting::AiModel) if value == OTHER_MODEL => {
                Outcome::Edit(Field::new(id, "").hint("Type the model's name · Enter saves · Esc cancels"))
            }
            Some(Setting::AiModel) => self.save(|c| c.ai_model = Some(value)),
            Some(Setting::Focus) => self.save(|c| c.focus = Some(value)),
            Some(Setting::Rank) => self.save(|c| c.rank = Some(value)),
            Some(Setting::Shots) => self.save(|c| c.shots = Some(value)),
            Some(Setting::Account) => self.save(|c| c.me = Some(value)),
            _ => Outcome::None,
        }
    }

    fn entered(&mut self, id: &str, text: &str) -> Outcome {
        let text = text.to_owned();
        if let Some(p) = id.strip_prefix(AI_KEY).and_then(ai::provider) {
            return self.save(|c| {
                if c.ai.as_deref() != Some(p.id) {
                    c.ai_model = None;
                }
                c.ai = Some(p.id.into());
                c.ai_key = Some(text);
            });
        }
        match Setting::from_id(id) {
            Some(Setting::AiModel) => self.save(|c| c.ai_model = Some(text)),
            Some(setting @ (Setting::ClipFolder | Setting::ReplayFolder)) => {
                let path = std::path::absolute(&text).unwrap_or_else(|_| PathBuf::from(&text));
                if setting == Setting::ReplayFolder {
                    return self.save(|c| c.demos_dir = Some(path));
                }
                if !path.is_dir() {
                    return Outcome::Error(format!("There's no folder at {}", path.display()));
                }
                self.save(|c| c.clips_dir = Some(path))
            }
            _ => Outcome::None,
        }
    }
}

/// A setting's value on the Config tab.
fn value(setting: Setting, cfg: &Config) -> Value {
    if let Some(on) = setting.switch(cfg) {
        return Value::Switch(on);
    }
    let text = |t: String| Value::Text(Style::default(), t);
    let unset = |t: &str| Value::Unset(t.to_owned());
    match setting {
        Setting::Ai => match cfg.ai.as_deref().and_then(ai::provider) {
            None => text("Off".into()),
            Some(p) if p.local => text(format!("{} · on this PC", p.name)),
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
                text(format!("{} · key ••••{tail}", p.name))
            }
        },
        Setting::ClipFolder => match &cfg.clips_dir {
            Some(d) => text(d.display().to_string()),
            None => unset("Not set"),
        },
        Setting::AiModel => match cfg.ai.as_deref().and_then(ai::provider) {
            Some(p) => text(ai::model_name(ai::chosen_model(p, cfg.ai_model.as_deref())).to_string()),
            None => text(String::new()),
        },
        Setting::Focus => text(habits::focus(cfg.focus.as_deref()).name.into()),
        Setting::Rank => match &cfg.rank {
            Some(r) => Value::Text(Style::fg(rank_color(Some(r))), r.clone()),
            None => unset("Not set"),
        },
        Setting::ReplayFolder => match cfg.demos_dir() {
            Some(d) => text(d.display().to_string()),
            None => unset("Not set"),
        },
        Setting::Shots => text(ShotStyle::parse(cfg.shots.as_deref()).name().into()),
        Setting::Account => match &cfg.me {
            Some(me) => text(me.clone()),
            None => unset("Not found yet"),
        },
        Setting::Watch | Setting::AutoUpdate => text(String::new()),
    }
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
        assert!(Setting::ALL.iter().all(|s| Setting::from_id(s.id()) == Some(*s)));
    }

    #[test]
    fn switches_and_unset_values() {
        let cfg = Config::default();
        assert_eq!(value(Setting::Watch, &cfg), Value::Switch(true));
        assert_eq!(value(Setting::Rank, &cfg), Value::Unset("Not set".into()));
        assert_eq!(
            value(Setting::Shots, &cfg),
            Value::Text(Style::default(), ShotStyle::parse(None).name().into())
        );
    }
}
