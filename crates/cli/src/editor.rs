//! The line editor behind the interactive shell: the buffer, the suggestion
//! menu and key handling. No terminal I/O here, so it is unit-tested.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One command the shell knows, with its flags.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub name: String,
    pub about: String,
    pub flags: Vec<FlagInfo>,
}

#[derive(Debug, Clone)]
pub struct FlagInfo {
    /// `--match`.
    pub long: String,
    pub help: String,
    /// `ID` when the flag takes a value.
    pub value: Option<String>,
}

/// A stored match, for completing `--match`.
#[derive(Debug, Clone)]
pub struct MatchInfo {
    pub id: String,
    /// `2026-10-19 21:04  Ascent`.
    pub label: String,
}

#[derive(Debug, Default)]
pub struct Context {
    pub commands: Vec<CommandInfo>,
    pub matches: Vec<MatchInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// What replaces the word being typed.
    pub value: String,
    /// What the menu shows.
    pub label: String,
    pub description: String,
}

#[derive(Debug, Default)]
pub struct Suggestions {
    /// Char index where the replaced word starts.
    pub start: usize,
    pub items: Vec<Suggestion>,
    /// The cursor is in the command name; Enter runs the highlighted command.
    pub command_word: bool,
}

/// Suggestions for the text before the cursor.
pub fn suggest(before: &str, ctx: &Context) -> Suggestions {
    let chars: Vec<char> = before.chars().collect();
    let start = chars.iter().rposition(|c| c.is_whitespace()).map_or(0, |i| i + 1);
    let word: String = chars[start..].iter().collect();
    if start == 0 {
        let Some(prefix) = word.strip_prefix('/') else {
            return Suggestions::default();
        };
        let prefix = prefix.to_lowercase();
        let starts = ctx.commands.iter().filter(|c| c.name.starts_with(&prefix));
        let contains = ctx
            .commands
            .iter()
            .filter(|c| !c.name.starts_with(&prefix) && c.name.contains(&prefix));
        let items = starts
            .chain(contains)
            .map(|c| Suggestion {
                value: format!("/{}", c.name),
                label: format!("/{}", c.name),
                description: c.about.clone(),
            })
            .collect();
        return Suggestions {
            start,
            items,
            command_word: true,
        };
    }

    let head: String = chars[..start].iter().collect();
    let tokens: Vec<&str> = head.split_whitespace().collect();
    let name = tokens[0].trim_start_matches('/').to_lowercase();
    let Some(cmd) = ctx.commands.iter().find(|c| c.name == name) else {
        return Suggestions::default();
    };
    let prev = tokens.last().copied().unwrap_or_default();
    let mut items = Vec::new();
    if let Some(flag) = cmd.flags.iter().find(|f| f.long == prev && f.value.is_some()) {
        if flag.long == "--match" {
            items = match_items(ctx, &word, "");
        }
    } else if word.is_empty() || word.starts_with('-') {
        if word.is_empty() && cmd.name == "packet" && tokens.len() == 1 {
            items = match_items(ctx, "", "--match ");
        }
        items.extend(
            cmd.flags
                .iter()
                .filter(|f| f.long.starts_with(&word) && !tokens.contains(&f.long.as_str()))
                .map(|f| Suggestion {
                    value: f.long.clone(),
                    label: match &f.value {
                        Some(v) => format!("{} <{v}>", f.long),
                        None => f.long.clone(),
                    },
                    description: f.help.clone(),
                }),
        );
    }
    Suggestions {
        start,
        items,
        command_word: false,
    }
}

fn match_items(ctx: &Context, prefix: &str, value_prefix: &str) -> Vec<Suggestion> {
    ctx.matches
        .iter()
        .filter(|m| m.id.starts_with(prefix))
        .map(|m| Suggestion {
            value: format!("{value_prefix}{}", m.id),
            label: m.id.chars().take(8).collect(),
            description: m.label.clone(),
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Submit(String),
    Exit,
    ClearScreen,
}

#[derive(Debug, Default)]
pub struct Editor {
    buf: Vec<char>,
    cursor: usize,
    selected: usize,
    menu_hidden: bool,
    history: Vec<String>,
    /// Index into `history` while browsing it.
    hist_pos: Option<usize>,
    /// The line being typed before browsing history.
    draft: Vec<char>,
    /// Ctrl+C on an empty line once; a second one exits.
    ctrl_c_armed: bool,
}

impl Editor {
    pub fn new(history: Vec<String>) -> Self {
        Editor {
            history,
            ..Editor::default()
        }
    }

    pub fn line(&self) -> String {
        self.buf.iter().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn ctrl_c_armed(&self) -> bool {
        self.ctrl_c_armed
    }

    /// The menu as it is drawn now: nothing while hidden or browsing history.
    pub fn suggestions(&self, ctx: &Context) -> Suggestions {
        if self.menu_hidden || self.hist_pos.is_some() {
            return Suggestions::default();
        }
        let before: String = self.buf[..self.cursor].iter().collect();
        suggest(&before, ctx)
    }

    /// The highlighted menu row, clamped to the menu.
    pub fn selected(&self, n: usize) -> usize {
        self.selected.min(n.saturating_sub(1))
    }

    /// Grey text after the cursor: the rest of the highlighted command name.
    pub fn ghost(&self, ctx: &Context) -> Option<String> {
        if self.cursor != self.buf.len() {
            return None;
        }
        let s = self.suggestions(ctx);
        if !s.command_word || s.items.is_empty() {
            return None;
        }
        let typed: String = self.buf[s.start..].iter().collect();
        let value = &s.items[self.selected(s.items.len())].value;
        value
            .strip_prefix(typed.as_str())
            .filter(|rest| !rest.is_empty())
            .map(str::to_owned)
    }

    /// Replace the line, cursor at the end.
    pub fn set_line(&mut self, s: &str) {
        self.buf.clear();
        self.cursor = 0;
        self.insert_str(s);
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            self.buf.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.edited();
    }

    fn edited(&mut self) {
        self.menu_hidden = false;
        self.selected = 0;
        self.hist_pos = None;
    }

    fn set(&mut self, line: &[char]) {
        self.buf = line.to_vec();
        self.cursor = self.buf.len();
    }

    /// Replace the word being completed with the highlighted suggestion.
    fn accept(&mut self, s: &Suggestions) {
        let item = &s.items[self.selected(s.items.len())];
        let tail: Vec<char> = self.buf[self.cursor..].to_vec();
        self.buf.truncate(s.start);
        self.buf.extend(item.value.chars());
        self.buf.push(' ');
        self.cursor = self.buf.len();
        self.buf.extend(tail);
        self.edited();
    }

    fn submit(&mut self) -> Action {
        let line = self.line().trim().to_owned();
        if !line.is_empty() && self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        self.buf.clear();
        self.cursor = 0;
        self.edited();
        Action::Submit(line)
    }

    fn history_step(&mut self, back: bool) {
        let n = self.history.len();
        let next = match (self.hist_pos, back) {
            (None, true) if n > 0 => {
                self.draft = self.buf.clone();
                Some(n - 1)
            }
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            (Some(_), false) => {
                let draft = std::mem::take(&mut self.draft);
                self.set(&draft);
                self.hist_pos = None;
                return;
            }
            _ => return,
        };
        if let Some(i) = next {
            let line: Vec<char> = self.history[i].chars().collect();
            self.set(&line);
            self.hist_pos = Some(i);
        }
    }

    pub fn handle(&mut self, key: KeyEvent, ctx: &Context) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let s = self.suggestions(ctx);
        let menu = !s.items.is_empty();
        if !(ctrl && key.code == KeyCode::Char('c')) {
            self.ctrl_c_armed = false;
        }
        match key.code {
            KeyCode::Enter => {
                if menu && s.command_word {
                    self.accept(&s);
                }
                return self.submit();
            }
            KeyCode::Tab if menu => self.accept(&s),
            KeyCode::Up if menu => self.selected = (self.selected(s.items.len()) + s.items.len() - 1) % s.items.len(),
            KeyCode::Down if menu => self.selected = (self.selected(s.items.len()) + 1) % s.items.len(),
            KeyCode::Up => self.history_step(true),
            KeyCode::Down => self.history_step(false),
            KeyCode::Esc => self.menu_hidden = true,
            KeyCode::Right if self.cursor == self.buf.len() && self.ghost(ctx).is_some() => self.accept(&s),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.buf.len()),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.buf.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.buf.remove(self.cursor);
                self.edited();
            }
            KeyCode::Delete if self.cursor < self.buf.len() => {
                self.buf.remove(self.cursor);
                self.edited();
            }
            KeyCode::Char(c) if ctrl => match c {
                'c' if self.buf.is_empty() => {
                    if self.ctrl_c_armed {
                        return Action::Exit;
                    }
                    self.ctrl_c_armed = true;
                }
                'c' | 'u' => {
                    self.buf.drain(..self.cursor);
                    self.cursor = 0;
                    if c == 'c' {
                        self.buf.clear();
                    }
                    self.edited();
                }
                'd' if self.buf.is_empty() => return Action::Exit,
                'a' => self.cursor = 0,
                'e' => self.cursor = self.buf.len(),
                'l' => return Action::ClearScreen,
                'w' => {
                    let mut i = self.cursor;
                    while i > 0 && self.buf[i - 1].is_whitespace() {
                        i -= 1;
                    }
                    while i > 0 && !self.buf[i - 1].is_whitespace() {
                        i -= 1;
                    }
                    self.buf.drain(i..self.cursor);
                    self.cursor = i;
                    self.edited();
                }
                _ => {}
            },
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::ALT) => {
                self.buf.insert(self.cursor, c);
                self.cursor += 1;
                self.edited();
            }
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        let flag = |long: &str, value: Option<&str>| FlagInfo {
            long: long.into(),
            help: format!("help for {long}"),
            value: value.map(Into::into),
        };
        let cmd = |name: &str, flags: Vec<FlagInfo>| CommandInfo {
            name: name.into(),
            about: format!("about {name}"),
            flags,
        };
        Context {
            commands: vec![
                cmd("scan", vec![flag("--keep-raw", None), flag("--retry-failed", None)]),
                cmd(
                    "packet",
                    vec![
                        flag("--match", Some("ID")),
                        flag("--me", Some("ME")),
                        flag("--out", Some("OUT")),
                    ],
                ),
                cmd("status", vec![]),
                cmd("trends", vec![flag("--last", Some("N"))]),
            ],
            matches: vec![
                MatchInfo {
                    id: "aaaa1111-0000".into(),
                    label: "2026-10-19  Ascent".into(),
                },
                MatchInfo {
                    id: "bbbb2222-0000".into(),
                    label: "2026-10-18  Lotus".into(),
                },
            ],
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(ed: &mut Editor, ctx: &Context, s: &str) {
        for c in s.chars() {
            ed.handle(key(KeyCode::Char(c)), ctx);
        }
    }

    fn values(s: &Suggestions) -> Vec<&str> {
        s.items.iter().map(|i| i.value.as_str()).collect()
    }

    #[test]
    fn command_names() {
        let c = ctx();
        assert!(suggest("", &c).items.is_empty());
        assert!(suggest("status", &c).items.is_empty(), "only after a slash");
        assert_eq!(values(&suggest("/", &c)).len(), 4);
        // Prefix matches first, then names containing the text.
        assert_eq!(values(&suggest("/s", &c)), ["/scan", "/status", "/trends"]);
        assert_eq!(values(&suggest("/tr", &c)), ["/trends"]);
        assert!(suggest("/st", &c).command_word);
    }

    #[test]
    fn arguments() {
        let c = ctx();
        // A bare `/packet ` offers the stored matches, then the flags.
        let s = suggest("/packet ", &c);
        assert!(!s.command_word);
        assert_eq!(
            values(&s),
            [
                "--match aaaa1111-0000",
                "--match bbbb2222-0000",
                "--match",
                "--me",
                "--out"
            ]
        );
        assert_eq!(s.items[0].label, "aaaa1111");
        assert_eq!(values(&suggest("/packet --match b", &c)), ["bbbb2222-0000"]);
        assert_eq!(values(&suggest("/packet --m", &c)), ["--match", "--me"]);
        // Used flags drop out; values of other flags get nothing.
        assert_eq!(values(&suggest("/scan --keep-raw ", &c)), ["--retry-failed"]);
        assert!(suggest("/trends --last ", &c).items.is_empty());
        assert!(suggest("/packet --out C:/x", &c).items.is_empty());
        assert!(suggest("/nope ", &c).items.is_empty());
    }

    #[test]
    fn enter_runs_the_highlighted_command() {
        let c = ctx();
        let mut ed = Editor::new(vec![]);
        typed(&mut ed, &c, "/st");
        assert_eq!(ed.ghost(&c).as_deref(), Some("atus"));
        assert_eq!(ed.handle(key(KeyCode::Enter), &c), Action::Submit("/status".into()));
        assert_eq!(ed.line(), "");

        // Down moves the highlight; Enter runs that one.
        typed(&mut ed, &c, "/s");
        ed.handle(key(KeyCode::Down), &c);
        assert_eq!(ed.handle(key(KeyCode::Enter), &c), Action::Submit("/status".into()));

        // Past the command name, Enter submits what was typed.
        typed(&mut ed, &c, "/trends --last 10");
        assert_eq!(
            ed.handle(key(KeyCode::Enter), &c),
            Action::Submit("/trends --last 10".into())
        );
        typed(&mut ed, &c, "/packet ");
        assert_eq!(ed.handle(key(KeyCode::Enter), &c), Action::Submit("/packet".into()));
    }

    #[test]
    fn tab_completes() {
        let c = ctx();
        let mut ed = Editor::new(vec![]);
        typed(&mut ed, &c, "/pa");
        ed.handle(key(KeyCode::Tab), &c);
        assert_eq!(ed.line(), "/packet ");
        ed.handle(key(KeyCode::Down), &c);
        ed.handle(key(KeyCode::Tab), &c);
        assert_eq!(ed.line(), "/packet --match bbbb2222-0000 ");
        // Right arrow at the end takes the grey hint.
        let mut ed = Editor::new(vec![]);
        typed(&mut ed, &c, "/tre");
        ed.handle(key(KeyCode::Right), &c);
        assert_eq!(ed.line(), "/trends ");
    }

    #[test]
    fn esc_hides_the_menu_until_the_next_edit() {
        let c = ctx();
        let mut ed = Editor::new(vec![]);
        typed(&mut ed, &c, "/st");
        ed.handle(key(KeyCode::Esc), &c);
        assert!(ed.suggestions(&c).items.is_empty());
        assert_eq!(ed.handle(key(KeyCode::Enter), &c), Action::Submit("/st".into()));
        typed(&mut ed, &c, "/s");
        ed.handle(key(KeyCode::Esc), &c);
        typed(&mut ed, &c, "c");
        assert_eq!(values(&ed.suggestions(&c)), ["/scan"]);
    }

    #[test]
    fn history_and_keys() {
        let c = ctx();
        let mut ed = Editor::new(vec!["/scan".into(), "/status".into()]);
        typed(&mut ed, &c, "dra");
        ed.handle(key(KeyCode::Up), &c);
        assert_eq!(ed.line(), "/status");
        assert!(ed.suggestions(&c).items.is_empty(), "no menu while browsing history");
        ed.handle(key(KeyCode::Up), &c);
        assert_eq!(ed.line(), "/scan");
        ed.handle(key(KeyCode::Down), &c);
        ed.handle(key(KeyCode::Down), &c);
        assert_eq!(ed.line(), "dra");

        ed.handle(ctrl('w'), &c);
        assert_eq!(ed.line(), "");
        typed(&mut ed, &c, "ab");
        ed.handle(key(KeyCode::Left), &c);
        ed.handle(key(KeyCode::Backspace), &c);
        assert_eq!((ed.line().as_str(), ed.cursor()), ("b", 0));

        // Ctrl+C clears a line; twice on an empty line exits.
        assert_eq!(ed.handle(ctrl('c'), &c), Action::None);
        assert_eq!(ed.line(), "");
        assert_eq!(ed.handle(ctrl('c'), &c), Action::None);
        assert!(ed.ctrl_c_armed());
        assert_eq!(ed.handle(ctrl('c'), &c), Action::Exit);
        assert_eq!(ed.handle(ctrl('d'), &c), Action::Exit);
    }
}
