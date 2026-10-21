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

### Added

- `vantage uninstall` removes Vantage and your data from your PC.
- A "Light terminal background" switch in /settings makes text readable on light terminals.

### Changed

- Times and dates are shown in your own time zone instead of UTC.
- /update is listed in the / menu and /help.
- Deathmatch and other modes without the spike are skipped instead of reviewed as draws.

### Fixed

- Screenshots from one match are no longer shown for another match on the same map and day.
- "Died with your ultimate ready" only counts when your ultimate really was ready.
- Damage and kills on your own teammates no longer count toward your stats.
- A death with no position data no longer counts as dying where nobody could trade.
- Self-kills no longer count as a lost opening duel, and draws are no longer counted as losses.
- Teams are worked out correctly after a teamkill, and players who never fought are counted as alive.
- Clips that start after a death are no longer used for its screenshots.
- A replay that couldn't be read because something else held vrfkit, or that was still downloading, is tried again later.
- A crash while saving no longer leaves the match database, the AI review or settings unreadable.
- One unreadable stored match no longer stops /habits, /plan and the rest from opening.
- An AI review that fails after you close /review now says so, and Ctrl+Enter waits for a running scan instead of doing nothing.
- Picking a clip folder no longer reopens a review you closed.
- /habits and /rank update when a new match arrives while they're open.
- The Fixes, Map and AI tabs keep the selected item's details on screen, and say when there's more below.
- Panel titles and key hints fit narrow terminals, and /habits fits 80 columns.
- Folders that don't exist can't be picked for replays or clips.

## [1.0.0] - 2026-10-21

### Added

- Debriefs that show why you died and the three things that cost you the most rounds.
- Habits, a focus habit, map plans and rank advice from your own replays.
- The replay reader updates itself when a new VALORANT patch comes out, with no Vantage update or restart.
- /release-notes shows what's new in each version.
- /tui default runs Vantage in your terminal's own scrollback, so its output stays after you quit; /tui fullscreen goes back.

[Unreleased]: https://github.com/Deaadman/vantage/compare/1.0.0...HEAD
[1.0.0]: https://github.com/Deaadman/vantage/releases/tag/1.0.0
