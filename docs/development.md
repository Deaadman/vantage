# Building from source

You need current stable Rust with the MSVC toolchain. Install Visual Studio's "Desktop development with C++" workload first.

```powershell
cargo build --release
cargo test
```

## How it's laid out

- `crates/core`: reads replays, analyses them, and does the coaching (debrief, habits, plans, ranks, clips and AI review).
- `crates/cli`: the `vantage` program: its commands, panels, settings and colours.

The shell itself (input box, menus, panels, logs, settings file, self-updater and installer) comes from [Abalone](https://github.com/Deaadman/abalone), a separate repository with nothing VALORANT-specific in it. To work on both at once, clone Abalone next to this repository and add this to `Cargo.toml`:

```toml
[patch.crates-io]
abalone = { path = "../abalone" }
```

`install.ps1` and `scripts/release-notes.ps1` are copied from Abalone. A test fails if they get out of sync. To copy them again, run `cargo test -p vantage write_install_script -- --ignored`.

## Tests

`cargo test` runs the unit tests. Three tests need extra files, and skip if they're missing:

| Test | Needs |
| --- | --- |
| `crates/core/tests/fixtures.rs` reads the replays in `tests/fixtures/public` | The vrfkit named in [`vrfkit.json`](../vrfkit.json), on PATH or set with `VANTAGE_VRFKIT=<path>` |
| `crates/core/tests/oracle.rs` checks one match against hand-checked numbers | Your own export, in `tests/data/oracle_export` or set with `VANTAGE_ORACLE_EXPORT` |
| `real_data_panels_open_and_fit` opens every screen with real matches | `VANTAGE_SMOKE_DATA=<data folder>` |

## Benchmark

This times each screen on made-up matches:

```powershell
cargo test --release -p vantage bench -- --ignored --nocapture
```

Set `BENCH_OUT=<file>` to also save the results as JSON.

## CI

[CI](../.github/workflows/ci.yml) runs on Windows for every push to `main` and every pull request. It checks:

- formatting, with `cargo fmt --check`
- that `CHANGELOG.md` follows the format below
- clippy, with warnings as errors
- every test, including the fixture test (CI downloads the vrfkit)
- that a release build works, and shows the exe's size
- that the [pictures in the docs](#the-pictures-in-the-docs) match what the code draws now
- that no crate in `Cargo.lock` has a [security advisory](https://rustsec.org), with `cargo audit`

Once a week, [Dependabot](../.github/dependabot.yml) opens a pull request for crates and actions with a newer version, with the minor and patch updates together. Merge it when CI passes. That includes [Abalone](https://crates.io/crates/abalone) when it has a release.

## Releasing

The [release workflow](../.github/workflows/release.yml) runs when a version tag is pushed:

1. In [`CHANGELOG.md`](../CHANGELOG.md), rename `## [Unreleased]` to the new version and date, for example `## [0.2.0] - 2026-11-02`. Add a new empty `## [Unreleased]` above it, and update the links at the bottom.
2. Set the same version in `crates/cli/Cargo.toml`. Commit both files.
3. Tag the commit and push the tag: `git tag 0.2.0`, then `git push origin 0.2.0`. Tags don't start with `v`.

The workflow then runs CI, checks the tag matches `crates/cli/Cargo.toml`, builds the exe and publishes the release. It stops if `CHANGELOG.md` has no section for that version.

### Writing the changelog

`CHANGELOG.md` follows [Keep a Changelog 2.0.0](https://keepachangelog.com/en/2.0.0/), but without a `v` in front of versions. CI checks the format and says what to fix.

Each version's notes appear in two places: the GitHub release, and the "what's new" list players see after updating. The app shows them as plain text, without headings, links or formatting. So write for players:

- Add each change under `## [Unreleased]` as you make it.
- One plain sentence per change, about what's different for players. "Clips shows each clip's length", not "Add length_s to Matched".
- Leave out changes players can't see, like refactors and tests, and vrfkit updates (those ship on their own, see below).
- Use full `https://` links. Links like `docs/review.md` break on the release page.

The format rules:

- Use only these headings, in this order, and skip empty ones: `### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`, `### Security`.
- Every line under a heading starts with `- `. A long entry can continue on the next line if it's indented.
- A version can start with a short summary before its first heading.
- Start a breaking change with `**Breaking:**`, and a security fix with its CVE number if it has one.
- Add ` [YANKED]` after the date of a pulled release. The workflow won't publish it.

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

The links at the bottom make each heading clickable. `[Unreleased]` compares the newest release with `HEAD`, each version compares with the one before, and the first links to its tag. Before the first release, `[Unreleased]` links to the commits on `main`.

To preview a release's notes, run `.\scripts\release-notes.ps1 -Version 0.2.0 -OutDir notes`. It writes the GitHub description to `notes/release-notes.md` and the in-app notes to `notes/whats-new.json`. Add `-Check` instead to only check the format, like CI does.

## vrfkit updates

[`vrfkit.json`](../vrfkit.json) says which vrfkit release Vantage uses: its download link, two SHA-256 hashes, the VALORANT versions it reads, the line players see when it arrives, and `min_vantage`, the oldest Vantage that works with it.

Installed copies of Vantage read this file from `main` when they check for updates. So merging a change to it updates vrfkit for everyone, without a Vantage release. Copies older than `min_vantage` wait until Vantage itself is updated.

You don't edit it by hand. The [vrfkit watch](../.github/workflows/vrfkit-watch.yml) workflow checks for a new vrfkit once a day. When it finds one, it:

1. runs [`scripts/vrfkit-bump.ps1`](../scripts/vrfkit-bump.ps1), which downloads and checks the release and rewrites `vrfkit.json`
2. opens a pull request, like "Update vrfkit to v0.4.0"
3. runs CI on it with the new vrfkit

If CI passes, edit the player line in `notes` if you want, then merge. To skip a release, close the pull request. The workflow needs **Allow GitHub Actions to create and approve pull requests** turned on, under Settings > Actions > General.

To try a release by hand, run `.\scripts\vrfkit-bump.ps1 -Tag v0.4.0`.

## The pictures in the docs

The pictures in `docs/images`, and the README's `launch.gif`, are Vantage's real screens drawn from made-up matches. When you change a screen the docs show, redraw them:

```powershell
.\scripts\redraw-docs-images.ps1
```

This runs the `redraw_docs_images` test in [`crates/cli/src/docs_pictures.rs`](../crates/cli/src/docs_pictures.rs), which redraws every picture and lists the ones that changed. Check those before you commit. CI fails if they're out of date. It needs the internet. Set `DOCS_IMAGES=<folder>` to save them somewhere else.

To add a picture, draw its screen in that test, then add it to a page with `![What it shows](images/<name>.svg)`.
