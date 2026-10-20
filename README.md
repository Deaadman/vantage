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

## Build

Windows, current stable Rust, MSVC toolchain (Visual Studio "Desktop development
with C++" workload; the parquet zstd decoder compiles C).

```
cargo build --release
```

## Use

Download the vrfkit v0.3.0 Windows release (valreview checks the exe's
SHA-256), then:

```
valreview init --vrfkit <path to vrfkit.exe>
valreview sync
valreview scan
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

`--me` takes an account-UUID prefix (4+ characters), a pawn id, or an agent
name. It errors if the selector matches more than one player.

Files live in `%LOCALAPPDATA%\valreview\data` (override with `--data-dir` or
`VALREVIEW_DATA_DIR`):

- `config.toml`: `me`, `vrfkit`, `demos_dir`, optional `vrfkit_sha256` to accept
  a deliberately upgraded vrfkit
- `catalog.json`: the valorant-api.com cache; a failed sync leaves it in place
- `valreview.db`: SQLite (matches, players, rounds, kills, deaths, scan_log)
- `tracks/<match>.parquet`: player positions at 4 Hz

## Layout

- `crates/core`: export loader, analysis, packet rendering. No CLI concerns.
- `crates/cli`: the `valreview` binary (clap commands only).

## Tests

```
cargo test
```

The oracle test (`crates/core/tests/oracle.rs`) checks every number in the plan's
test oracle. It needs the Lotus export, which is personal data and not committed:
set `VALREVIEW_ORACLE_EXPORT=<export_dir>` or put it at `tests/data/oracle_export`.
Without it the test skips.
