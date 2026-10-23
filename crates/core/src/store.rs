//! `vantage.db`: matches, players, rounds, kills, death facts, and the scan log.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::analysis::Analysis;
use crate::catalog::Catalog;
use crate::context::MatchContext;
use crate::deaths::is_traded;
use crate::export::MatchReview;
use crate::game::Game;
use crate::time::now_rfc3339;

pub const STORE_SCHEMA_VERSION: i64 = 4;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS matches (
    match_id TEXT PRIMARY KEY,
    source_file TEXT NOT NULL,
    source_size INTEGER NOT NULL,
    replay_build TEXT NOT NULL,
    map_url TEXT,
    map_name TEXT,
    played_at TEXT,
    duration_ms INTEGER NOT NULL,
    rounds INTEGER NOT NULL,
    vrfkit_version TEXT NOT NULL,
    processed_at TEXT NOT NULL,
    context_json TEXT NOT NULL,
    UNIQUE (source_file, source_size)
);
CREATE TABLE IF NOT EXISTS players (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    pawn INTEGER NOT NULL,
    agent_id TEXT,
    agent TEXT NOT NULL,
    role TEXT,
    side INTEGER,
    kills INTEGER NOT NULL,
    deaths INTEGER NOT NULL,
    ping_median INTEGER,
    reconnects INTEGER NOT NULL,
    PRIMARY KEY (match_id, subject)
);
CREATE TABLE IF NOT EXISTS rounds (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    PRIMARY KEY (match_id, number)
);
CREATE TABLE IF NOT EXISTS kills (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    time_ms INTEGER NOT NULL,
    round INTEGER,
    killer_pawn INTEGER NOT NULL,
    victim_pawn INTEGER NOT NULL,
    self_kill INTEGER NOT NULL,
    traded_within_3s INTEGER NOT NULL,
    PRIMARY KEY (match_id, idx)
);
-- One row per death of every player; fact_json is a deaths.jsonl line.
CREATE TABLE IF NOT EXISTS deaths (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    time_ms INTEGER NOT NULL,
    victim_subject TEXT NOT NULL,
    round INTEGER,
    fact_json TEXT NOT NULL,
    PRIMARY KEY (match_id, time_ms, victim_subject)
);
CREATE INDEX IF NOT EXISTS deaths_by_subject ON deaths (victim_subject);
-- Files scan should not retry: failed exports and duplicates of stored matches.
CREATE TABLE IF NOT EXISTS scan_log (
    source_file TEXT NOT NULL,
    source_size INTEGER NOT NULL,
    status TEXT NOT NULL,
    detail TEXT NOT NULL,
    vrfkit_version TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (source_file, source_size)
);
";

/// Added in schema 2.
const SCHEMA_V2: &str = "
-- One row per player per round; fact_json is a rounds.jsonl line.
CREATE TABLE IF NOT EXISTS round_facts (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    round INTEGER NOT NULL,
    fact_json TEXT NOT NULL,
    PRIMARY KEY (match_id, subject, round)
);
";

/// Added in schema 3.
const SCHEMA_V3: &str = "
-- One MatchReview (everything an export shows) per player per match.
CREATE TABLE IF NOT EXISTS reviews (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    review_json TEXT NOT NULL,
    PRIMARY KEY (match_id, subject)
);
";

/// Schema 4: `matches.mode`. SQLite has no `ADD COLUMN IF NOT EXISTS`, so a
/// file whose version row lags its tables is checked first. Matches stored
/// earlier have no raw export to read the mode from, so they stay without
/// one: the rounds can't tell Swiftplay from Spike Rush or a surrender.
fn add_mode_column(conn: &Connection) -> Result<()> {
    let has_mode = conn
        .prepare("SELECT 1 FROM pragma_table_info('matches') WHERE name = 'mode'")?
        .exists([])?;
    if !has_mode {
        conn.execute_batch("ALTER TABLE matches ADD COLUMN mode TEXT")?;
    }
    Ok(())
}

/// (match_id, played_at, map name or path)
pub type MatchRow = (String, Option<String>, Option<String>);

pub struct Store {
    conn: Connection,
}

/// What the store already knows about a replay file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    New,
    Stored {
        match_id: String,
    },
    Logged {
        status: String,
        detail: String,
        /// The vrfkit that produced this outcome.
        vrfkit_version: String,
    },
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Store { conn };
        store
            .migrate()
            .with_context(|| format!("preparing {}", path.display()))?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let store = Store {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    /// Creates or upgrades the tables. It's one transaction, and every
    /// statement is `IF NOT EXISTS`, so a crash part way leaves the file as it
    /// was, and a file left half made by an older Vantage is finished off.
    fn migrate(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let has_meta: bool = tx
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        // No version row: a new file, or one an older Vantage left half made.
        let version: i64 = if has_meta {
            tx.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|v| v.parse().context("bad schema_version in meta"))
            .transpose()?
            .unwrap_or(0)
        } else {
            0
        };
        if version > STORE_SCHEMA_VERSION {
            bail!(
                "database schema {version} is newer than this version of Vantage ({STORE_SCHEMA_VERSION}); update Vantage"
            );
        }
        // Matches stored before a migration lack the new rows; their raw
        // exports are gone, so they stay without them.
        if version < 1 {
            tx.execute_batch(SCHEMA)?;
        }
        if version < 2 {
            tx.execute_batch(SCHEMA_V2)?;
        }
        if version < 3 {
            tx.execute_batch(SCHEMA_V3)?;
        }
        if version < 4 {
            add_mode_column(&tx)?;
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [STORE_SCHEMA_VERSION.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn file_state(&self, source_file: &str, source_size: u64) -> Result<FileState> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT match_id FROM matches WHERE source_file = ?1 AND source_size = ?2",
                params![source_file, source_size as i64],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(match_id) = stored {
            return Ok(FileState::Stored { match_id });
        }
        let logged: Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT status, detail, vrfkit_version FROM scan_log WHERE source_file = ?1 AND source_size = ?2",
                params![source_file, source_size as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(
            logged.map_or(FileState::New, |(status, detail, vrfkit_version)| FileState::Logged {
                status,
                detail,
                vrfkit_version,
            }),
        )
    }

    pub fn has_match(&self, match_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM matches WHERE match_id = ?1", [match_id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn log_file(
        &self,
        source_file: &str,
        source_size: u64,
        status: &str,
        detail: &str,
        vrfkit_version: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO scan_log (source_file, source_size, status, detail, vrfkit_version, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                source_file,
                source_size as i64,
                status,
                detail,
                vrfkit_version,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Stores one match and every player's death facts in one transaction.
    pub fn insert_match(
        &mut self,
        game: &Game,
        catalog: &Catalog,
        source_file: &str,
        source_size: u64,
        vrfkit_version: &str,
    ) -> Result<MatchContext> {
        let ctx = MatchContext::new(game, catalog);
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM scan_log WHERE source_file = ?1 AND source_size = ?2",
            params![source_file, source_size as i64],
        )?;
        tx.execute(
            "INSERT INTO matches (match_id, source_file, source_size, replay_build, map_url, map_name, played_at,
                                  duration_ms, rounds, vrfkit_version, processed_at, context_json, mode)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                ctx.match_id,
                source_file,
                source_size as i64,
                ctx.replay_build,
                ctx.map_url,
                ctx.map_name,
                ctx.played_at,
                ctx.duration_ms as i64,
                ctx.rounds as i64,
                vrfkit_version,
                now_rfc3339(),
                serde_json::to_string(&ctx)?,
                ctx.mode,
            ],
        )?;
        for team in &ctx.teams {
            for p in &team.players {
                tx.execute(
                    // A player listed twice (after a reconnect) keeps their first row.
                    "INSERT OR IGNORE INTO players (match_id, subject, pawn, agent_id, agent, role, side, kills, deaths, ping_median, reconnects)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        ctx.match_id,
                        p.subject,
                        p.pawn,
                        p.agent_id,
                        p.agent,
                        p.role,
                        team.side,
                        p.kills as i64,
                        p.deaths as i64,
                        p.ping_median,
                        p.reconnects as i64
                    ],
                )?;
            }
        }
        for r in &game.rounds {
            tx.execute(
                "INSERT OR IGNORE INTO rounds (match_id, number, start_ms) VALUES (?1, ?2, ?3)",
                params![ctx.match_id, r.number, r.start_ms],
            )?;
        }
        for k in &game.kills {
            tx.execute(
                "INSERT INTO kills (match_id, idx, time_ms, round, killer_pawn, victim_pawn, self_kill, traded_within_3s)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    ctx.match_id,
                    k.index as i64,
                    k.time_ms,
                    game.round(k).map(|r| r.number),
                    k.killer,
                    k.victim,
                    k.is_self_kill(),
                    is_traded(game, k)
                ],
            )?;
        }
        let analysis = Analysis::new(game, catalog);
        for p in game.players() {
            let review = MatchReview::new(&analysis, &ctx, p);
            tx.execute(
                "INSERT INTO reviews (match_id, subject, review_json) VALUES (?1, ?2, ?3)",
                params![ctx.match_id, p.subject, serde_json::to_string(&review)?],
            )?;
            for f in &review.death_facts {
                tx.execute(
                    "INSERT INTO deaths (match_id, time_ms, victim_subject, round, fact_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![ctx.match_id, f.time_ms, p.subject, f.round, serde_json::to_string(f)?],
                )?;
            }
            for f in &review.round_facts {
                tx.execute(
                    "INSERT INTO round_facts (match_id, subject, round, fact_json) VALUES (?1, ?2, ?3, ?4)",
                    params![ctx.match_id, p.subject, f.round, serde_json::to_string(f)?],
                )?;
            }
        }
        tx.commit()?;
        Ok(ctx)
    }

    pub fn match_count(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM matches", [], |r| r.get::<_, i64>(0))? as usize)
    }

    /// Account UUIDs present in every stored match, when there are at least two
    /// matches. With random teammates, exactly one subject is almost surely you.
    pub fn subjects_in_every_match(&self) -> Result<(usize, Vec<String>)> {
        let n = self.match_count()?;
        if n < 2 {
            return Ok((n, Vec::new()));
        }
        let mut stmt = self.conn.prepare(
            "SELECT subject FROM players GROUP BY subject HAVING COUNT(DISTINCT match_id) = ?1 ORDER BY subject",
        )?;
        let subjects = stmt
            .query_map([n as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok((n, subjects))
    }

    /// Every account UUID seen in any stored match.
    pub fn known_subjects(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT subject FROM players ORDER BY subject")?;
        let subjects = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(subjects)
    }

    /// Accounts in at least `min` stored matches, most matches first:
    /// (subject, matches, agents played, most played first).
    pub fn accounts(&self, min: usize) -> Result<Vec<(String, usize, Vec<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT subject, COUNT(DISTINCT match_id) AS n FROM players
             GROUP BY subject HAVING n >= ?1 ORDER BY n DESC, subject",
        )?;
        let rows = stmt
            .query_map([min as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut agents = self
            .conn
            .prepare("SELECT agent FROM players WHERE subject = ?1 GROUP BY agent ORDER BY COUNT(*) DESC, agent")?;
        rows.into_iter()
            .map(|(subject, n)| {
                let played = agents
                    .query_map([&subject], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                Ok((subject, n as usize, played))
            })
            .collect()
    }

    /// One player's stored review of one match.
    pub fn review(&self, match_id: &str, subject: &str) -> Result<Option<MatchReview>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT review_json FROM reviews WHERE match_id = ?1 AND subject = ?2",
                params![match_id, subject],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|j| parse_review(&j)).transpose()
    }

    /// Every stored review of one player, oldest match first.
    pub fn reviews_of(&self, subject: &str) -> Result<Vec<MatchReview>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.review_json FROM reviews r JOIN matches m USING (match_id)
             WHERE r.subject = ?1 ORDER BY COALESCE(m.played_at, m.processed_at), m.match_id",
        )?;
        let rows = stmt
            .query_map([subject], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // One review this version can't read is left out, not the whole history.
        Ok(rows
            .iter()
            .filter_map(|j| {
                parse_review(j)
                    .inspect_err(|e| abalone::log::write("WARN", &format!("skipped a stored review: {e:#}")))
                    .ok()
            })
            .collect())
    }

    /// Stored matches, newest first: (match_id, played_at, map).
    pub fn matches(&self) -> Result<Vec<MatchRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT match_id, played_at, COALESCE(map_name, map_url) FROM matches
             ORDER BY COALESCE(played_at, processed_at) DESC, match_id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every stored match, newest first, with how it went for `subject`
    /// (agent, score and K/D are missing when they weren't in it).
    pub fn my_matches(&self, subject: &str) -> Result<Vec<MyMatch>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.match_id, m.played_at, COALESCE(m.map_name, m.map_url), m.mode, p.agent, p.kills, p.deaths,
                (SELECT COUNT(*) FROM round_facts r WHERE r.match_id = m.match_id AND r.subject = ?1
                    AND json_extract(r.fact_json, '$.won') = 1),
                (SELECT COUNT(*) FROM round_facts r WHERE r.match_id = m.match_id AND r.subject = ?1
                    AND json_extract(r.fact_json, '$.won') = 0)
             FROM matches m LEFT JOIN players p ON p.match_id = m.match_id AND p.subject = ?1
             ORDER BY COALESCE(m.played_at, m.processed_at) DESC, m.match_id",
        )?;
        let rows = stmt
            .query_map([subject], |r| {
                Ok(MyMatch {
                    id: r.get(0)?,
                    played_at: r.get(1)?,
                    map: r.get(2)?,
                    mode: r.get(3)?,
                    agent: r.get(4)?,
                    kills: r.get::<_, Option<i64>>(5)?.map(|v| v as usize),
                    deaths: r.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                    won: r.get::<_, i64>(7)? as usize,
                    lost: r.get::<_, i64>(8)? as usize,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Players of one match: (subject, pawn, agent).
    pub fn players_of(&self, match_id: &str) -> Result<Vec<(String, u32, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT subject, pawn, agent FROM players WHERE match_id = ?1 ORDER BY pawn")?;
        let rows = stmt
            .query_map([match_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn counts(&self) -> Result<StoreCounts> {
        let one = |sql: &str| -> Result<usize> { Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize) };
        Ok(StoreCounts {
            matches: one("SELECT COUNT(*) FROM matches")?,
            kills: one("SELECT COUNT(*) FROM kills")?,
            deaths: one("SELECT COUNT(*) FROM deaths")?,
            logged: one("SELECT COUNT(*) FROM scan_log")?,
        })
    }
}

/// One match from one player's side, for lists and pickers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MyMatch {
    pub id: String,
    pub played_at: Option<String>,
    /// Map name, or its path when the catalog didn't know it.
    pub map: Option<String>,
    /// Game mode, e.g. `Comp/Unrated/Premier` or `Swiftplay`.
    pub mode: Option<String>,
    pub agent: Option<String>,
    pub kills: Option<usize>,
    pub deaths: Option<usize>,
    pub won: usize,
    pub lost: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreCounts {
    pub matches: usize,
    pub kills: usize,
    pub deaths: usize,
    pub logged: usize,
}

fn parse_review(json: &str) -> Result<MatchReview> {
    let r: MatchReview = serde_json::from_str(json).context("reading a stored review")?;
    if r.schema_version != crate::export::REVIEW_SCHEMA_VERSION {
        bail!(
            "stored review schema {} differs from this version's {}",
            r.schema_version,
            crate::export::REVIEW_SCHEMA_VERSION
        );
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_store_and_log() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.file_state("a.vrf", 10).unwrap(), FileState::New);
        s.log_file("a.vrf", 10, "failed", "boom", "v0.3.0").unwrap();
        assert!(matches!(s.file_state("a.vrf", 10).unwrap(), FileState::Logged { .. }));
        // A different size is a different file.
        assert_eq!(s.file_state("a.vrf", 11).unwrap(), FileState::New);
        assert_eq!(s.subjects_in_every_match().unwrap(), (0, vec![]));
    }

    #[test]
    fn accounts_by_matches() {
        let s = Store::open_in_memory().unwrap();
        let add = |m: &str, subject: &str, agent: &str| {
            s.conn
                .execute(
                    "INSERT OR IGNORE INTO matches (match_id, source_file, source_size, replay_build, duration_ms,
                     rounds, vrfkit_version, processed_at, context_json) VALUES (?1, ?1, 1, 'b', 0, 0, 'v', 't', '{}')",
                    [m],
                )
                .unwrap();
            s.conn
                .execute(
                    "INSERT INTO players (match_id, subject, pawn, agent, kills, deaths, reconnects)
                     VALUES (?1, ?2, 0, ?3, 0, 0, 0)",
                    [m, subject, agent],
                )
                .unwrap();
        };
        for (m, agent) in [("m1", "Sova"), ("m2", "Fade"), ("m3", "Sova")] {
            add(m, "me", agent);
        }
        add("m1", "duo", "Jett");
        add("m2", "duo", "Jett");
        add("m3", "once", "Omen");
        let accounts = s.accounts(2).unwrap();
        assert_eq!(
            accounts,
            [
                ("me".to_owned(), 3, vec!["Sova".to_owned(), "Fade".to_owned()]),
                ("duo".to_owned(), 2, vec!["Jett".to_owned()]),
            ]
        );
    }

    #[test]
    fn reopening_keeps_schema() {
        let path = std::env::temp_dir().join(format!("vantage-store-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Store::open(&path)
            .unwrap()
            .log_file("a.vrf", 1, "failed", "x", "v0.3.0")
            .unwrap();
        let s = Store::open(&path).unwrap();
        assert_eq!(s.counts().unwrap().logged, 1);
        drop(s);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn half_made_files_are_finished_off() {
        let path = std::env::temp_dir().join(format!("vantage-store-half-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // A crash after the tables but before the version row.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        drop(conn);
        let s = Store::open(&path).unwrap();
        s.log_file("a.vrf", 1, "failed", "x", "v0.3.0").unwrap();
        drop(s);
        // A crash after schema 3's table but before the version was raised.
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE meta SET value = '2' WHERE key = 'schema_version'", [])
            .unwrap();
        drop(conn);
        assert_eq!(Store::open(&path).unwrap().counts().unwrap().logged, 1);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn upgrading_leaves_stored_matches_without_a_mode() {
        let path = std::env::temp_dir().join(format!("vantage-store-modes-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // A schema 3 file with a Swiftplay-length, a full-length and a roundless match.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.execute_batch(
            "INSERT INTO meta VALUES ('schema_version', '3');
             INSERT INTO matches VALUES ('swift', 'a.vrf', 1, 'b', NULL, NULL, NULL, 0, 8, 'v', 'now', '{}');
             INSERT INTO matches VALUES ('full', 'b.vrf', 1, 'b', NULL, NULL, NULL, 0, 24, 'v', 'now', '{}');
             INSERT INTO matches VALUES ('none', 'c.vrf', 1, 'b', NULL, NULL, NULL, 0, 0, 'v', 'now', '{}');",
        )
        .unwrap();
        drop(conn);
        let modes = |s: &Store| -> Vec<(String, Option<String>)> {
            s.my_matches("me")
                .unwrap()
                .into_iter()
                .map(|m| (m.id, m.mode))
                .collect()
        };
        let s = Store::open(&path).unwrap();
        // The rounds can't tell Swiftplay from Spike Rush, so no mode is guessed.
        let want = vec![
            ("full".to_owned(), None),
            ("none".to_owned(), None),
            ("swift".to_owned(), None),
        ];
        assert_eq!(modes(&s), want);
        drop(s);
        // A crash after the column but before the version was raised: the
        // column is kept, and so is a mode stored since.
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE meta SET value = '3' WHERE key = 'schema_version'", [])
            .unwrap();
        conn.execute("UPDATE matches SET mode = 'Swiftplay' WHERE match_id = 'swift'", [])
            .unwrap();
        drop(conn);
        let mut want = want;
        want[2].1 = Some("Swiftplay".to_owned());
        assert_eq!(modes(&Store::open(&path).unwrap()), want);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_unreadable_review_is_skipped() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute_batch(
                "INSERT INTO matches VALUES ('m1', 'a.vrf', 1, 'b', NULL, NULL, '2026-01-01', 0, 0, 'v', 'now', '{}', NULL);
                 INSERT INTO reviews VALUES ('m1', 'me', '{not json');",
            )
            .unwrap();
        assert!(s.reviews_of("me").unwrap().is_empty());
    }
}
