//! `valreview.db`: matches, players, rounds, kills, death facts, and the scan log.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::analysis::Analysis;
use crate::catalog::Catalog;
use crate::context::MatchContext;
use crate::deaths::is_traded;
use crate::game::Game;
use crate::packet::MatchReview;
use crate::time::now_rfc3339;

pub const STORE_SCHEMA_VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE matches (
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
CREATE TABLE players (
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
CREATE TABLE rounds (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    PRIMARY KEY (match_id, number)
);
CREATE TABLE kills (
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
CREATE TABLE deaths (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    time_ms INTEGER NOT NULL,
    victim_subject TEXT NOT NULL,
    round INTEGER,
    fact_json TEXT NOT NULL,
    PRIMARY KEY (match_id, time_ms, victim_subject)
);
CREATE INDEX deaths_by_subject ON deaths (victim_subject);
-- Files scan should not retry: failed exports and duplicates of stored matches.
CREATE TABLE scan_log (
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
CREATE TABLE round_facts (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    round INTEGER NOT NULL,
    fact_json TEXT NOT NULL,
    PRIMARY KEY (match_id, subject, round)
);
";

/// Added in schema 3.
const SCHEMA_V3: &str = "
-- One MatchReview (everything a packet shows) per player per match.
CREATE TABLE reviews (
    match_id TEXT NOT NULL REFERENCES matches(match_id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    review_json TEXT NOT NULL,
    PRIMARY KEY (match_id, subject)
);
";

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

    fn migrate(&self) -> Result<()> {
        let has_meta: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !has_meta {
            self.conn.execute_batch(SCHEMA)?;
            self.conn.execute_batch(SCHEMA_V2)?;
            self.conn.execute_batch(SCHEMA_V3)?;
            self.conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                [STORE_SCHEMA_VERSION.to_string()],
            )?;
            return Ok(());
        }
        let version: i64 = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| {
                r.get::<_, String>(0)
            })?
            .parse()
            .context("bad schema_version in meta")?;
        if version > STORE_SCHEMA_VERSION {
            bail!("database schema {version} is newer than this valreview ({STORE_SCHEMA_VERSION}); update valreview");
        }
        // Matches stored before a migration lack the new rows; their raw
        // exports are gone, so they stay without them.
        if version < 2 {
            self.conn.execute_batch(SCHEMA_V2)?;
        }
        if version < 3 {
            self.conn.execute_batch(SCHEMA_V3)?;
        }
        self.conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
            [STORE_SCHEMA_VERSION.to_string()],
        )?;
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
                                  duration_ms, rounds, vrfkit_version, processed_at, context_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            ],
        )?;
        for team in &ctx.teams {
            for p in &team.players {
                tx.execute(
                    "INSERT INTO players (match_id, subject, pawn, agent_id, agent, role, side, kills, deaths, ping_median, reconnects)
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
        rows.iter().map(|j| parse_review(j)).collect()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreCounts {
    pub matches: usize,
    pub kills: usize,
    pub deaths: usize,
    pub logged: usize,
}

fn parse_review(json: &str) -> Result<MatchReview> {
    let r: MatchReview = serde_json::from_str(json).context("reading a stored review")?;
    if r.schema_version != crate::packet::REVIEW_SCHEMA_VERSION {
        bail!(
            "stored review schema {} differs from this valreview's {}",
            r.schema_version,
            crate::packet::REVIEW_SCHEMA_VERSION
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
    fn reopening_keeps_schema() {
        let path = std::env::temp_dir().join(format!("valreview-store-{}.db", std::process::id()));
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
}
