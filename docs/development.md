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
| `crates/core/tests/fixtures.rs` scans the public replays in `tests/fixtures/public` | The vrfkit in [`vrfkit.json`](../vrfkit.json), set with `VANTAGE_VRFKIT=<path>` or on PATH |
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
- a check that `CHANGELOG.md` is written in the format below
- clippy, with warnings as errors
- every test, with the vrfkit in `vrfkit.json` downloaded so the fixture scan runs
- a release build, with the exe's size in the run's summary

## Releasing

The [release workflow](../.github/workflows/release.yml) is turned off for now. Once it's on, a release goes like this:

1. In [`CHANGELOG.md`](../CHANGELOG.md), rename `## [Unreleased]` to the new version and today's date, for example `## [0.2.0] - 2026-11-02`. Add a fresh, empty `## [Unreleased]` above it, and update the links at the bottom (see below).
2. Set the same version in `crates/cli/Cargo.toml`, and commit both.
3. Tag the commit with the version and push the tag: `git tag 0.2.0`, then `git push origin 0.2.0`. Tags have no `v` in front; the app adds one when it shows a version.

The workflow runs CI, checks the tag matches `crates/cli/Cargo.toml`, builds the exe and publishes the release. It stops if `CHANGELOG.md` has no section for the version.

### Writing the changelog

`CHANGELOG.md` follows [Keep a Changelog 2.0.0](https://keepachangelog.com/en/2.0.0/), except that versions and tags have no `v`. CI checks the format on every push, and the error says what to fix.

- Add each change to `## [Unreleased]` as you make it, newest version first below that.
- Group entries under these headings, in this order, leaving out any with nothing in them: `### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`, `### Security`. No other headings.
- Every line under a heading is a `- ` entry. A long entry can carry on to the next line if that line is indented.
- A version can open with a short summary before its first heading.
- Start an entry that breaks something people rely on with `**Breaking:**`. Start a security fix with its CVE number, if it has one.
- Leave out vrfkit updates. They reach players on their own (see "vrfkit updates" below).
- Mark a release that was pulled with ` [YANKED]` after its date. The workflow won't publish it.

Each version's section is used twice:

- **The GitHub release description**: the section as you wrote it, followed by GitHub's own notes (merged pull requests and a link to the full list of changes).
- **What's new in the app**: each entry is one line players see on the first launch after updating. The app shows them as plain text, so links, `code` and **bold** marks are removed, and the headings aren't shown.

So write for players, not for developers:

- One entry per change, as a plain sentence about what's different for them. "Clips shows each clip's length", not "Add length_s to Matched".
- Leave out changes players can't see, like refactors and tests.
- Use full `https://` links. A link like `docs/review.md` works in the repo but not on the release page.

```markdown
## [Unreleased]

## [0.2.0] - 2026-11-02

### Added

- Pick which AI model reviews your screenshots in /settings.

### Fixed

- Clips shows each clip's length.

## [0.1.0] - 2026-10-21

### Added

- The first release.

[Unreleased]: https://github.com/Deaadman/vantage/compare/0.2.0...HEAD
[0.2.0]: https://github.com/Deaadman/vantage/compare/0.1.0...0.2.0
[0.1.0]: https://github.com/Deaadman/vantage/releases/tag/0.1.0
```

The links at the bottom make each heading clickable. `[Unreleased]` compares the newest release with `HEAD`, each version compares with the one before it, and the oldest links to its own tag. Before the first release, `[Unreleased]` links to the commits on `main`.

To see what a release would get, run `.\scripts\release-notes.ps1 -Version 0.2.0 -OutDir notes`. It writes the description to `notes/release-notes.md` and the in-app notes to `notes/whats-new.json`. `.\scripts\release-notes.ps1 -Check` only checks the format, as CI does.

## vrfkit updates

[`vrfkit.json`](../vrfkit.json) names the vrfkit release Vantage uses: where to download it, its two SHA-256 hashes, the VALORANT builds it reads, the line players see when it arrives, and `min_vantage`, the oldest Vantage that can read its output. Each build of Vantage has the file built in, CI tests with the release it names, and installed copies fetch the newest one from `main` when they check for updates. So merging a change to it updates vrfkit everywhere, with no Vantage release. Installs on an older Vantage than `min_vantage` wait for a Vantage update instead.

You don't edit it by hand. Once a day the [vrfkit watch](../.github/workflows/vrfkit-watch.yml) workflow looks for a new vrfkit release. When there is one, it:

1. runs [`scripts/vrfkit-bump.ps1`](../scripts/vrfkit-bump.ps1), which downloads the release, checks it against GitHub's record of the file, reads the builds it supports from vrfkit's README, and rewrites `vrfkit.json`
2. opens a pull request, "Update vrfkit to v0.4.0", with the line players will see and vrfkit's own notes
3. starts CI on it, which runs every test with the new vrfkit

Check that CI passed, reword the player line in `notes` if you want, and merge. To skip a release, close its pull request. The workflow needs **Allow GitHub Actions to create and approve pull requests**, under Settings > Actions > General.

When Vantage checks for updates and `main` names a newer vrfkit, it downloads it, checks both hashes and that it starts, then switches to it straight away. The one before stays installed. To try a release by hand, run `.\scripts\vrfkit-bump.ps1 -Tag v0.4.0`.

## The pictures in the docs

The pictures in `docs/images` are Vantage's real screens, drawn from made-up matches and saved as SVG. The README's `launch.gif` is the same screens, played in order. When you change a screen the docs show, redraw them:

```powershell
.\scripts\redraw-docs-images.ps1
```

It runs the ignored test `redraw_docs_images` in [`crates/cli/src/docs_pictures.rs`](../crates/cli/src/docs_pictures.rs), which rewrites all 14 pictures and the GIF, then lists the ones that changed. Look over those before you commit. It needs the internet, for the map catalog and Lotus's minimap. Set `DOCS_IMAGES=<folder>` to write the pictures somewhere else.

To add a picture, draw its screen in that test, then use it in a page as `![What it shows](images/<name>.svg)`.
