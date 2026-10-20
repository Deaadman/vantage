# valreview

![](https://img.shields.io/badge/Windows-x64-blue?style=flat-square) ![](https://img.shields.io/badge/vrfkit-v0.3.0-blueviolet?style=flat-square) [![](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)

valreview reads your own Valorant replays and shows you how each match went: your stats against the rest of the lobby, what to work on (with the rounds behind each point), every round and death, and where you died on the map. It tracks trends across matches, and it can export a match as facts for an AI to review. It runs locally, for personal use, and never touches the game or the Riot client.

Replays are decoded by [vrfkit](https://github.com/yakisoba0728/vrfkit), which the installer sets up for you.

## Get started

> [!NOTE]
> Windows x64 only. The PowerShell installer is the only supported way to install valreview.

1. Install valreview:

    **Windows (PowerShell):**
    ```powershell
    irm https://raw.githubusercontent.com/Deaadman/valreview/main/install.ps1 | iex
    ```

    This installs valreview and the vrfkit release it uses to `%LOCALAPPDATA%\valreview`, checks both against their SHA-256, and adds `%LOCALAPPDATA%\valreview\bin` to your PATH.

2. Download your replays in the Valorant client, open a new terminal and run `valreview`.

The interactive shell looks after itself. When it opens, it scans your replay folder, and works out which account is yours. It then keeps watching the folder, so a replay you download while it's open is processed as soon as the download finishes. Agent, map and weapon names are refreshed from valorant-api.com once a day, and valreview updates itself (see below).

Type `/` to see the commands, or `/help` for an overview, every command and the keyboard shortcuts:

| Command | Does |
| --- | --- |
| `/review` | Opens a match (newest by default): Overview, Work on, Rounds, Deaths and Map tabs |
| `/summary` | Sums up all your matches on one map (`--map Pearl`, default the newest match's map): what keeps repeating |
| `/export` | Writes the same match as `export.md`, `deaths.jsonl` and `rounds.jsonl` for an AI to review |
| `/trends` | Charts your damage, K/D, headshots, KST and first deaths across matches, and the death spots that repeat |
| `/matches` | Lists stored matches by day, newest first |
| `/scan` | Processes the replay folder now (`--retry-failed` tries failed replays again) |
| `/settings` | Shows versions and setup (Status), and changes the replay folder, vrfkit and your account, and turns automatic updates, folder watching and keeping raw exports on or off (Config) |

Each one also works as a plain command, for example `valreview export --match <id>`. Add `--help` to any of them for its flags. Outside the shell, `valreview review` and `valreview summary` print their Overview and Work on tabs as text. `valreview sync` and `valreview update` also still work by hand, although the shell runs them for you.

### Reviewing a match

`/review` compares you with the other nine players, using the stored review of everyone in the match:

- **Overview** ranks your damage per round, K/D, headshots, KST (kill, survived or traded), first kills and first deaths against the lobby. It also shows the scoreboard, what went well, and your top three things to work on.
- **Work on** ranks what stood out, each with its evidence. Examples are a spot where you keep dying, an enemy who won most of your fights, deaths with your ult ready, deaths without hitting your killer, weapons or ranges where you lose, and fighting on with a rifle when the round is lost. Enter jumps to the rounds involved.
- **Rounds** has one line per round (side, result, buy, kills, damage, how you died) and the detail for the selected round.
- **Deaths** lists every death with the full picture: the fight, your aim, what you held, who was near, whether you were traded.
- **Map** draws the minimap in the terminal with your deaths, the sites and repeated spots. ↑↓ steps through the deaths.

### Summing up a map

`/summary` looks across every match you have on one map, because a mistake that repeats is a habit rather than a bad game. Each finding says in how many of the matches it shows up, for example "You die at A Hut in 4 of 5 matches", and lists the evidence match by match. It uses the same panel as `/review`, with these differences:

- **Overview** totals your rounds won (attack and defense), damage, headshots, first kills and first deaths against the lobbies you played in, and lists every match.
- **Work on** has the cross-match findings: death spots that repeat across matches, enemy agents that beat you whoever plays them, ult-ready deaths, deaths without hitting your killer, headshots below the lobby, a side or a site that keeps losing (for example retakes on B), and late first fights.
- **Matches** lists each match. Enter opens its full `/review`, and Esc comes back to the summary.
- **Deaths** and **Map** show every death on that map across the matches.

Patterns need a few matches to mean anything. With fewer than 3 on a map, the summary says so and points you to `/review`.

Rounds count from 1 here, as in the game. The export counts from 0. Findings describe what the replay measured; the export's caveats (for example a less-verified game build) apply to them too.

## Updates

valreview updates itself. Each time you open the shell it checks GitHub for a new release, then downloads and installs it in the background, along with the vrfkit version that release needs. The bottom right shows "Updated to … · Restart to apply". The version you're using keeps working, and the next launch starts the new one.

Run `valreview update` to update straight away. To turn off the background check, turn off Automatic updates in `/settings` (it saves `auto_update = false` in `config.toml`) or set the environment variable `VALREVIEW_DISABLE_AUTOUPDATER=1`; `valreview update` still works. `valreview settings` shows the result of the last check.

## Your data

Everything stays on your machine in `%LOCALAPPDATA%\valreview\data` (`--data-dir` or `VALREVIEW_DATA_DIR` to move it): the config, a SQLite database of match facts, and 4 Hz movement tracks. Raw replay exports are deleted once processed. The only network calls are the valorant-api.com catalog for agent and map names, and downloads of valreview and vrfkit releases from GitHub.

## Reporting bugs

File a [GitHub issue](https://github.com/Deaadman/valreview/issues). Include the output of `valreview settings`.

## Development

Building from source needs current stable Rust with the MSVC toolchain (the Visual Studio "Desktop development with C++" workload).

```powershell
cargo build --release
cargo test
```

- `crates/core`: export loader, analysis and `export.md` rendering.
- `crates/cli`: the `valreview` binary and the interactive shell.

`crates/core/tests/fixtures.rs` runs the whole pipeline on the public replays in `tests/fixtures/public` and needs vrfkit (`VALREVIEW_VRFKIT=<path>` or on PATH). The oracle test needs a personal export at `VALREVIEW_ORACLE_EXPORT` or `tests/data/oracle_export`. Both skip without their inputs.
