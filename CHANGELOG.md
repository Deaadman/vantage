# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/2.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!--
A version's section is its GitHub release description, and its entries are
what the app shows after an update, so write them for players: one plain
sentence each, about what changed for them.

- Add entries under "## [Unreleased]" as you go, grouped under Added,
  Changed, Deprecated, Removed, Fixed or Security.
- To release, rename it to "## [0.2.0] - YYYY-MM-DD" (no v), add a fresh
  "## [Unreleased]" above it, and update the links at the bottom.

See docs/development.md, "Releasing". CI checks this file's format.
-->

## [Unreleased]

## [1.0.1] - 2026-10-22

### Fixed

- Screenshots work straight after ffmpeg is installed, even when winget installed it for the whole computer.
- A replay that was still downloading when Vantage looked for new ones is now read once it finishes, and the bottom left shows the download until then.

## [1.0.0] - 2026-10-21

### Added

- The first release.

[Unreleased]: https://github.com/Deaadman/vantage/compare/1.0.1...HEAD
[1.0.1]: https://github.com/Deaadman/vantage/compare/1.0.0...1.0.1
[1.0.0]: https://github.com/Deaadman/vantage/releases/tag/1.0.0
