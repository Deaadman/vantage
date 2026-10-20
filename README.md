# Vantage

![](https://img.shields.io/badge/Windows-x64-blue?style=flat-square) ![](https://img.shields.io/badge/vrfkit-v0.3.0-blueviolet?style=flat-square) [![](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)

Vantage is a VALORANT coach that reads your own replays. After each match it tells you why you died, the three things that cost you the most rounds, and the moments that decided them. Over time it tracks the habits you keep repeating, and it tells you what wins games at your rank. It runs locally, for personal use, and never touches the game or the Riot client.

Replays are decoded by [vrfkit](https://github.com/yakisoba0728/vrfkit), which the installer sets up for you.

## Get started

> [!NOTE]
> Windows x64 only. The PowerShell installer is the only supported way to install Vantage.

1. Install Vantage:

    **Windows (PowerShell):**
    ```powershell
    irm https://raw.githubusercontent.com/Deaadman/vantage/main/install.ps1 | iex
    ```

    This installs Vantage and the vrfkit release it uses to `%LOCALAPPDATA%\vantage`, checks both against their SHA-256, and adds `%LOCALAPPDATA%\vantage\bin` to your PATH.

2. Download your replays in the VALORANT client, open a new terminal and run `vantage`.

3. Set your rank in `/settings`. Replays don't include it, and `/rank` and `/plan` use it.

The shell looks after itself. When it opens, it scans your replay folder and works out which account is yours. It then keeps watching the folder, so a replay you download while it's open is processed as soon as the download finishes. Agent, map and weapon names are refreshed from valorant-api.com once a day, and Vantage updates itself (see below).

Type `/` to see the commands, or `/help` for an overview and the keyboard shortcuts:

| Command | Does |
| --- | --- |
| `/review` | The debrief for a match (your newest by default, `--match` for another) |
| `/plan` | A game plan for one map: where you win, where you lose, and what to do on each side (`--map Lotus`, default the map you play most) |
| `/habits` | The mistakes you keep making, and which are getting better |
| `/focus` | Picks one habit to work on (`/focus untraded`). Every new match checks it first |
| `/rank` | What wins games at your rank, and how you compare with the players in your games |
| `/matches` | Your matches, newest first (`--all` for more than 10) |
| `/scan` | Looks for new replays now (`--retry-failed` tries failed replays again) |
| `/settings` | Versions and setup (Status), and your rank, replay folder, clip folder, focus, AI review and updates (Config) |

Each one also works as a plain command, for example `vantage review --match <id>`. Add `--help` to any of them for its flags. Outside the shell, `vantage review` and `vantage plan` print as text.

### Reviewing a match

`/review` has eight tabs:

- **Debrief**: how the match went in a few lines, and what to keep doing.
- **Fixes**: the three mistakes that cost you the most rounds, each with the rounds behind it and what to try instead.
- **Moments**: the fights that decided rounds, won and lost.
- **Rounds**: one line per round, and the detail for the selected round.
- **Map**: the minimap in the terminal with your deaths and your best and worst spots.
- **Clips**: your clips matched to your deaths. Vantage finds the folder of Medal, NVIDIA ShadowPlay, Xbox Game Bar, AMD Adrenalin, Outplayed or SteelSeries Moments, or you can type your own.
- **Screenshots**: a screenshot of the seconds before each death, taken from its clip. These need ffmpeg (`winget install ffmpeg`) and are saved in `Pictures\Vantage\Clips`.
- **AI**: optional. An AI looks at the screenshots for what the replay can't show, like where you stood and where your crosshair was. It uses Claude, ChatGPT or Gemini with your own API key, or Ollama running on your PC. Each match is reviewed once, and the answer is saved.

### Habits

`/habits` counts eight habits in every match: dying where nobody can trade, going in first with no ability, shooting while moving, a crosshair too low, peeking again after a kill, dying with your ult ready, reloading in the open, and fighting rounds that are lost. It shows which are getting better. Pick one with `/focus`, and each new match tells you how you did on it.

## Updates

Vantage updates itself. Each time you open the shell it checks GitHub for a new release, then downloads and installs it in the background, along with the vrfkit version that release needs. The bottom right shows "Update available! Restart to apply". The version you're using keeps working, and the next launch starts the new one. The first launch after an update shows what's new.

Run `vantage update` to update straight away. To turn off the background check, turn off Automatic updates in `/settings` (it saves `auto_update = false` in `config.toml`) or set the environment variable `VANTAGE_DISABLE_AUTOUPDATER=1`; `vantage update` still works. `vantage settings` shows the result of the last check.

## Your data

Everything stays on your machine in `%LOCALAPPDATA%\vantage\data` (`--data-dir` or `VANTAGE_DATA_DIR` to move it): the config, a SQLite database of match facts, and 4 Hz movement tracks. Raw replay exports are deleted once processed, and screenshots are saved in `Pictures\Vantage\Clips`.

The network calls are the valorant-api.com catalog for agent and map names, and downloads of Vantage and vrfkit releases from GitHub. If you turn on AI review with Claude, ChatGPT or Gemini, the match's facts and its screenshots are sent to that provider when you ask for a review. Your API key stays in `config.toml`. Ollama runs on your PC and sends nothing.

## Reporting bugs

File a [GitHub issue](https://github.com/Deaadman/vantage/issues). Include the output of `vantage settings`.

## Development

Building from source needs current stable Rust with the MSVC toolchain (the Visual Studio "Desktop development with C++" workload).

```powershell
cargo build --release
cargo test
```

- `crates/core`: the export loader, the analysis and the coaching (debrief, habits, plans, ranks, clips and AI review).
- `crates/cli`: the `vantage` binary and the interactive shell.

`crates/core/tests/fixtures.rs` runs the whole pipeline on the public replays in `tests/fixtures/public` and needs vrfkit (`VANTAGE_VRFKIT=<path>` or on PATH). The oracle test needs a personal export at `VANTAGE_ORACLE_EXPORT` or `tests/data/oracle_export`. `VANTAGE_SMOKE_DATA=<data folder>` opens every panel on real scanned matches. All three skip without their inputs.
