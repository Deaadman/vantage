# valreview

Reads your own Valorant replays (through [vrfkit](https://github.com/yakisoba0728/vrfkit)
exports) and writes facts about your deaths in a form an AI can review.
Local and personal use only: it never touches the game or the Riot client.

## Status

Phase 1: loader, team inference, per-death facts, `packet.md` and `deaths.jsonl`
from one vrfkit export. No network, no database yet.

## Build

Windows, current stable Rust, MSVC toolchain (Visual Studio "Desktop development
with C++" workload; the parquet zstd decoder compiles C).

```
cargo build --release
```

## Use

Export a replay with vrfkit v0.3.0, then:

```
vrfkit export <file.vrf> --out <export_dir>
valreview players <export_dir>
valreview packet <export_dir> --me <selector> [--out <dir>]
```

`--me` takes an account-UUID prefix (4+ characters), a pawn id, or an agent
name. It errors if the selector matches more than one player.

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
