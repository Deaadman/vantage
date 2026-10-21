# Building from source

You need current stable Rust with the MSVC toolchain. Install Visual Studio's "Desktop development with C++" workload first.

```powershell
cargo build --release
cargo test
```

## How it's laid out

- `crates/core`: reading replays, the analysis, and the coaching (debrief, habits, plans, ranks, clips and AI review).
- `crates/cli`: the `vantage` program and its interactive shell.

## Tests

`cargo test` runs the unit tests. Three kinds of test need extra files and skip without them:

| Test | Needs |
| --- | --- |
| `crates/core/tests/fixtures.rs` scans the public replays in `tests/fixtures/public` | vrfkit v0.3.0, set with `VANTAGE_VRFKIT=<path>` or on PATH |
| `crates/core/tests/oracle.rs` checks a hand-verified match | A personal export at `VANTAGE_ORACLE_EXPORT` or `tests/data/oracle_export` |
| `real_data_panels_open_and_fit` opens every screen on real matches | `VANTAGE_SMOKE_DATA=<data folder>` |

## Benchmark

A benchmark times the work behind each screen on made-up matches:

```powershell
cargo test --release -p vantage bench -- --ignored --nocapture
```

Set `BENCH_OUT=<file>` to also save the results as JSON.

## CI

Every push to `main` and every pull request runs [CI](../.github/workflows/ci.yml) on Windows:

- `cargo fmt --check`
- clippy, with warnings as errors
- every test, with the pinned vrfkit downloaded so the fixture scan runs
- a release build, with the exe's size in the run's summary

The [release workflow](../.github/workflows/release.yml) runs CI first and only publishes from a `v*` tag. It's turned off for now.

## The pictures in the docs

The pictures in `docs/images` are Vantage's real screens, drawn from made-up matches and saved as SVG. When you change a screen the docs show, redraw them:

```powershell
.\scripts\redraw-docs-images.ps1
```

It runs the ignored test `redraw_docs_images` in [`crates/cli/src/docs_pictures.rs`](../crates/cli/src/docs_pictures.rs), which rewrites all 14 pictures, then lists the ones that changed. Look over those before you commit. It needs the internet, for the map catalog and Lotus's minimap. Set `DOCS_IMAGES=<folder>` to write the pictures somewhere else.

To add a picture, draw its screen in that test, then use it in a page as `![What it shows](images/<name>.svg)`.
