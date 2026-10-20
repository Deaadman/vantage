//! The `/help` panel: tabs for an overview and the commands, drawn in place
//! of the input box until Esc.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::CommandInfo;
use crate::shell::{ACCENT, BRIGHT, GREY, Row, Style, wrap};

const TABS: [&str; 2] = ["General", "Commands"];
const ISSUES: &str = "https://github.com/Deaadman/valreview/issues";
const HOME: &str = "https://github.com/Deaadman/valreview";

/// What the General tab says: (style, text) lines; long ones wrap.
const GENERAL: &[(bool, &str)] = &[
    (
        false,
        "valreview reads your own Valorant replays and writes facts about your play in a form an AI can review. Everything stays on this computer.",
    ),
    (false, ""),
    (true, "Automatic"),
    (
        false,
        "  Scans your replay folder when it opens, then keeps watching it: a replay you download is processed once it finishes.",
    ),
    (
        false,
        "  Refreshes agent, map and weapon names from valorant-api.com once a day.",
    ),
    (
        false,
        "  Installs new valreview releases in the background; restart to use them.",
    ),
    (false, ""),
    (true, "Reviewing a match"),
    (
        false,
        "  /review shows how your newest match went (or /review --match <id>): your stats against the lobby, what to work on with the rounds behind it, every round and death, and your deaths on the map.",
    ),
    (
        false,
        "  /summary sums up all your matches on one map (or /summary --map Pearl): what keeps repeating across them, each match, and every death on the map. Enter on a match opens its review.",
    ),
    (
        false,
        "  /export writes the same match as export.md, deaths.jsonl and rounds.jsonl to give to an AI.",
    ),
    (
        false,
        "  /trends compares your last 20 matches with the 20 before and lists death spots that repeat.",
    ),
    (false, ""),
    (true, "Keys"),
    (false, "  Enter            run the line, or the highlighted command"),
    (false, "  Tab, →           complete the highlighted suggestion"),
    (false, "  ↑ ↓              move in the menu; otherwise walk history"),
    (false, "  Esc              hide the menu"),
    (false, "  PgUp PgDn        scroll the output"),
    (false, "  Ctrl+U  Ctrl+W   clear to the start, delete a word"),
    (false, "  Ctrl+L           clear the screen"),
    (false, "  Ctrl+C           clear the line; twice on an empty line quits"),
    (false, ""),
    (false, "Every command takes --help, for example /export --help."),
];

pub enum HelpAction {
    None,
    Close,
    /// Close and put this in the input line.
    Use(String),
}

#[derive(Default)]
pub struct HelpPanel {
    tab: usize,
    /// Highlighted command on the Commands tab.
    selected: usize,
    /// First row shown on the General tab.
    scroll: usize,
}

impl HelpPanel {
    pub fn key(&mut self, key: KeyEvent, commands: &[CommandInfo]) -> HelpAction {
        let n = commands.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return HelpAction::Close,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return HelpAction::Close,
            KeyCode::Left | KeyCode::BackTab => self.tab = (self.tab + TABS.len() - 1) % TABS.len(),
            KeyCode::Right | KeyCode::Tab => self.tab = (self.tab + 1) % TABS.len(),
            KeyCode::Up if self.tab == 1 => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down if self.tab == 1 => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp if self.tab == 1 => self.selected = self.selected.saturating_sub(5),
            KeyCode::PageDown if self.tab == 1 => self.selected = (self.selected + 5).min(n.saturating_sub(1)),
            KeyCode::Home if self.tab == 1 => self.selected = 0,
            KeyCode::End if self.tab == 1 => self.selected = n.saturating_sub(1),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll += 1,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll += 10,
            KeyCode::Enter if self.tab == 1 => {
                if let Some(c) = commands.get(self.selected) {
                    return HelpAction::Use(format!("/{} ", c.name));
                }
            }
            _ => {}
        }
        HelpAction::None
    }

    /// The panel for a `w`-column screen: as tall as its content, at most `h` rows.
    pub fn rows(&mut self, commands: &[CommandInfo], w: usize, h: usize) -> Vec<Row> {
        let mut top: Vec<Row> = vec![vec![(Style::fg(ACCENT), "─".repeat(w.saturating_sub(1)))], vec![]];
        let mut tabs: Row = vec![(Style::fg(ACCENT).bold(), "  Help".into())];
        for (i, name) in TABS.iter().enumerate() {
            tabs.push((Style::default(), "  ".into()));
            let style = if i == self.tab {
                Style {
                    fg: Some(crossterm::style::Color::Black),
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

        let bottom: Vec<Row> = vec![
            vec![],
            vec![
                (Style::fg(GREY), "  For more help: ".into()),
                (Style::default(), HOME.into()),
            ],
            vec![],
            vec![(
                Style::fg(GREY),
                format!(
                    "  {} · ←→ tabs · Esc closes · something wrong? {ISSUES}",
                    if self.tab == 1 {
                        "↑↓ move · Enter uses the command"
                    } else {
                        "↑↓ scroll"
                    }
                ),
            )],
        ];
        // As tall as the longer tab needs, so switching tabs keeps the height.
        let general = general_lines(w);
        let needed = general.len().max(commands_height(commands.len()));
        let body_h = needed.min(h.saturating_sub(top.len() + bottom.len())).max(1);
        let mut body = if self.tab == 0 {
            self.general(general, body_h)
        } else {
            self.commands(commands, body_h)
        };
        body.resize(body_h, vec![]);
        let mut rows = top;
        rows.extend(body);
        rows.extend(bottom);
        rows.truncate(h);
        rows
    }

    fn general(&mut self, lines: Vec<Row>, h: usize) -> Vec<Row> {
        let max = lines.len().saturating_sub(h);
        self.scroll = self.scroll.min(max);
        let mut rows: Vec<Row> = lines.into_iter().skip(self.scroll).take(h).collect();
        if self.scroll < max
            && let Some(last) = rows.last_mut()
        {
            *last = vec![(Style::fg(GREY), format!("  ↓ {} more", max - self.scroll))];
        }
        rows
    }

    /// Two rows per command (name, then what it does), the highlighted one kept in view.
    fn commands(&mut self, commands: &[CommandInfo], h: usize) -> Vec<Row> {
        let n = commands.len();
        self.selected = self.selected.min(n.saturating_sub(1));
        let mut rows: Vec<Row> = vec![vec![(Style::default(), "  Browse commands".into())]];
        // Everything, or as many as fit with a row for each "more" line.
        let fit = if commands_height(n) <= h {
            n
        } else {
            (h.saturating_sub(3) / 2).max(1)
        };
        let first = self.selected.saturating_sub(fit - 1).min(n.saturating_sub(fit));
        if first > 0 {
            rows.push(vec![(Style::fg(GREY), format!("    ↑ {first} more"))]);
        } else {
            rows.push(vec![]);
        }
        for (i, c) in commands.iter().enumerate().skip(first).take(fit) {
            let (lead, style) = if i == self.selected {
                ("  ❯ ", Style::fg(ACCENT))
            } else {
                ("    ", Style::default())
            };
            rows.push(vec![(style, lead.into()), (style, format!("/{}", c.name))]);
            rows.push(vec![(Style::fg(GREY), format!("    {}", c.about))]);
        }
        let below = n.saturating_sub(first + fit);
        if below > 0 {
            rows.push(vec![(Style::fg(GREY), format!("    ↓ {below} more"))]);
        }
        rows
    }
}

/// The General tab's text wrapped to the screen.
fn general_lines(w: usize) -> Vec<Row> {
    let text_w = w.saturating_sub(6).max(10);
    let mut lines = Vec::new();
    for (bold, text) in GENERAL {
        let style = if *bold {
            Style::default().bold()
        } else {
            Style::default()
        };
        for piece in wrap(text, text_w) {
            lines.push(vec![(style, format!("  {piece}"))]);
        }
    }
    lines
}

/// Rows the Commands tab needs to list `n` commands: a title, the row above
/// the list, and two rows per command.
fn commands_height(n: usize) -> usize {
    2 + 2 * n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            kind: KeyEventKind::Press,
            ..KeyEvent::new(code, KeyModifiers::NONE)
        }
    }

    fn commands(n: usize) -> Vec<CommandInfo> {
        (0..n)
            .map(|i| CommandInfo {
                name: format!("cmd{i}"),
                about: format!("does {i}"),
                flags: vec![],
            })
            .collect()
    }

    fn text(rows: &[Row]) -> String {
        rows.iter()
            .map(|r| r.iter().map(|(_, s)| s.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn fits_its_content_and_the_screen() {
        // Few commands: as tall as the General text needs, on either tab, with no room to spare.
        let c = commands(3);
        let mut p = HelpPanel::default();
        let natural = p.rows(&c, 80, 200).len();
        assert!(natural < 200);
        p.key(key(KeyCode::Right), &c);
        assert_eq!(p.rows(&c, 80, 200).len(), natural);
        // Many commands: capped to the screen, never taller.
        let c = commands(60);
        for h in [8, 14, 40] {
            assert!(p.rows(&c, 80, h).len() <= h);
        }
        assert_eq!(p.rows(&c, 80, 40).len(), 40);
    }

    #[test]
    fn keeps_the_selected_command_in_view_and_uses_it() {
        let c = commands(30);
        let mut p = HelpPanel::default();
        p.key(key(KeyCode::Tab), &c);
        for _ in 0..20 {
            p.key(key(KeyCode::Down), &c);
        }
        let shown = text(&p.rows(&c, 80, 20));
        assert!(shown.contains("❯ /cmd20"));
        assert!(shown.contains("↑") && shown.contains("↓"));
        assert!(matches!(p.key(key(KeyCode::Enter), &c), HelpAction::Use(s) if s == "/cmd20 "));
        assert!(matches!(p.key(key(KeyCode::Esc), &c), HelpAction::Close));
    }
}
