# valreview

Reads your own Valorant replays (through [vrfkit](https://github.com/yakisoba0728/vrfkit)
exports) and writes facts about your deaths in a form an AI can review.
Local and personal use only: it never touches the game or the Riot client.

## Status

- Phase 1: loader, team inference, per-death facts, `packet.md` and `deaths.jsonl`
  from one vrfkit export.
- Phase 2: `valreview sync` caches agents, maps (minimap projection, callouts),
  weapons and seasons from valorant-api.com. Deaths get `minimap_uv` and
  `nearest_callout`. Everything works offline; without a cache, agent names come
  from a built-in table and map facts are left out.
- Phase 3: `valreview init` and `valreview scan`. Scan exports each new replay
  with the pinned vrfkit, stores match context, kills and every player's death
  facts in SQLite, keeps 4 Hz movement tracks, and deletes the raw export.
- Phase 4: groups B to F. Gunfights (damage, hit regions, wallbangs, weapons,
  magazine at death, duels, accuracy and moving shots from magazine counts),
  positioning (trades, forward rank at first contact, area time, rotations),
  utility (casts per slot, ultimates, casts before contact or death), economy
  (credits, loadout, buy types, team alignment, force/bonus/anti-eco results)
  and rounds (sides, results, first kills, man advantage, plants, retakes,
  clutches). `packet` also writes `rounds.jsonl`; the database keeps every
  player's round facts.

  Not built, because the replay does not support it on 13.08: abilities left
  unused at death (charge state is not decoded) and reload state at death (the
  magazine count is reported instead). Derived facts state their caveats in
  `packet.md`.

  Checked against the game's own numbers on the oracle replay: every player's
  `AggregateKills`/`AggregateDeaths` match the kill events, and damage dealt and
  taken match the CombatReport exactly.
- Phase 5: `valreview trends` aggregates 34 facts across scanned matches and
  compares the last N matches with the N before (default 20). Changes are ranked
  by a two-proportion z (rates) or Welch's t (per round) times how consistent they
  are, and only when both windows have 20+ matches and 30+ events; everything
  else is labelled "not enough data". Death spots that repeat across matches on
  a map are listed with match and round evidence. `scan` now stores a full review
  per player, so `valreview packet --match <id>` works after the raw export is gone.
- Phase 6: `valreview status` checks vrfkit, the catalog, the database and the
  replay folder. Every command prints one line when the live game (from the
  catalog) is past the builds vrfkit decodes, or when the catalog is over 30 days
  old. `scan` reads each replay's build with `vrfkit inspect` first and skips
  unsupported builds without exporting; failed or unsupported files are retried
  automatically after a vrfkit change. A crash on one replay is reported and the
  scan continues. Teams use spike carriers as well as kills. Schema versions are
  in the database (`meta`), `packet.md`, `trends.md` and every JSON line.

## Build

Windows, current stable Rust, MSVC toolchain (Visual Studio "Desktop development
with C++" workload; the parquet zstd decoder compiles C).

```
cargo build --release
```

## Use

Download the vrfkit v0.3.0 Windows release (valreview checks the exe's
SHA-256) and point valreview at it once:

```
valreview init --vrfkit <path to vrfkit.exe>
```

### Interactive shell

Run `valreview` with no command. On launch it fetches the valorant-api.com
catalog if there is none, scans the replay folder, sets your account once it is
the only one in every match (2+), and lists your recent matches. Then it takes
`/commands`: typing `/` opens a menu of commands with what each does, and after a
command name it suggests that command's flags and, for `/packet`, your stored
matches.

| Key | Does |
| --- | --- |
| Enter | Runs the highlighted command while typing its name; otherwise runs the line as typed |
| Tab, or → at the end | Completes the highlighted suggestion |
| ↑ ↓ | Moves in the menu; with no menu, walks history (`history.txt` in the data dir) |
| Esc | Hides the menu until the next edit |
| Ctrl+U, Ctrl+W, Ctrl+L | Clear to start, delete a word, clear the screen |
| Ctrl+C | Clears the line; twice on an empty line quits (so does Ctrl+D or `/exit`) |

Every command below works in the shell as `/name` with the same flags
(`/packet --help` for help). `/packet` with no match reviews the newest one.
Double quotes group a path with spaces; backslashes stay as typed.

### Commands

```
valreview init --vrfkit <path to vrfkit.exe>
valreview sync
valreview scan
valreview matches
valreview packet [--match <id prefix>] [--me <selector>] [--out <dir>]
valreview trends [--last 20] [--out <dir>]
```

Run `scan` after each session, once you have downloaded the replays in the
client. It skips files it has already processed (by file name and size), reports
corrupt or unsupported replays once and then skips them (`--retry-failed` to try
again), and deletes each raw export (`--keep-raw` keeps them). After two or more
matches, `valreview init` detects your account as the one present in every match;
`--me` sets it explicitly.

Single-export commands, for a directory written by `vrfkit export`:

```
valreview players <export_dir>
valreview packet <export_dir> --me <selector> [--out <dir>]
```

`packet` writes `packet.md` (for an AI reviewer), `deaths.jsonl` (one line per
death, schema 3) and `rounds.jsonl` (one line per round).

`--me` takes an account-UUID prefix (4+ characters), a pawn id, or an agent
name. It errors if the selector matches more than one player.

Files live in `%LOCALAPPDATA%\valreview\data` (override with `--data-dir` or
`VALREVIEW_DATA_DIR`):

- `config.toml`: `me`, `vrfkit`, `demos_dir`, optional `vrfkit_sha256` to accept
  a deliberately upgraded vrfkit
- `catalog.json`: the valorant-api.com cache; a failed sync leaves it in place
- `valreview.db`: SQLite schema 3 (matches, players, rounds, kills, deaths, round_facts,
  reviews, scan_log). Matches stored by an older schema keep their rows but lack
  the newer ones; their raw exports are gone, so they cannot be re-derived.
- `tracks/<match>.parquet`: player positions at 4 Hz

## Layout

- `crates/core`: export loader, analysis, packet rendering. No CLI concerns.
- `crates/cli`: the `valreview` binary: clap commands, and the interactive shell
  (`shell.rs` draws it with crossterm; `editor.rs` holds the line, menu and keys,
  unit-tested without a terminal).

## Tests

```
cargo test
```

- `crates/core/tests/fixtures.rs` runs the whole pipeline (scan, rescan, `me`
  detection, packets, trends, a corrupt file) on the three public replays in
  `tests/fixtures/public` (vrfkit's CI fixtures, MIT; see the NOTICE there). It
  needs vrfkit: set `VALREVIEW_VRFKIT=<path to vrfkit.exe>` or put it on PATH;
  without it the test skips.

The oracle test (`crates/core/tests/oracle.rs`) checks every number in the plan's
test oracle. It needs the Lotus export, which is personal data and not committed:
set `VALREVIEW_ORACLE_EXPORT=<export_dir>` or put it at `tests/data/oracle_export`.
Without it the test skips.
