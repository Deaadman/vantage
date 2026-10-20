# valreview

![](https://img.shields.io/badge/Windows-x64-blue?style=flat-square) ![](https://img.shields.io/badge/vrfkit-v0.3.0-blueviolet?style=flat-square) [![](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)

valreview reads your own Valorant replays and writes facts about your deaths, gunfights, positioning, utility, economy and rounds in a form an AI can review. It tracks trends across matches and points out death spots that keep repeating. It runs locally, for personal use, and never touches the game or the Riot client.

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

The interactive shell scans your replay folder, works out which account is yours and lists your recent matches. Type `/` to see the commands:

| Command | Does |
| --- | --- |
| `/scan` | Processes new replays into the local database |
| `/matches` | Lists stored matches |
| `/packet` | Writes `packet.md`, `deaths.jsonl` and `rounds.jsonl` for a match (newest by default) for an AI to review |
| `/trends` | Compares your last 20 matches with the 20 before and lists repeated death spots |
| `/status` | Checks vrfkit, the catalog, the database and the replay folder |
| `/sync` | Refreshes agents, maps and weapons from valorant-api.com |
| `/update` | Installs the newest valreview now |

Each one also works as a plain command, for example `valreview packet --match <id>`. Add `--help` to any of them for its flags.

## Updates

valreview updates itself. Each time you open the shell it checks GitHub for a new release, then downloads and installs it in the background, along with the vrfkit version that release needs. The bottom right shows "Updated to … · Restart to apply". The version you're using keeps working, and the next launch starts the new one.

Run `valreview update` to update straight away. To turn off the background check, set `auto_update = false` in `config.toml` or the environment variable `VALREVIEW_DISABLE_AUTOUPDATER=1`; `valreview update` still works. `valreview status` shows the result of the last check.

## Your data

Everything stays on your machine in `%LOCALAPPDATA%\valreview\data` (`--data-dir` or `VALREVIEW_DATA_DIR` to move it): the config, a SQLite database of match facts, and 4 Hz movement tracks. Raw replay exports are deleted once processed. The only network calls are the valorant-api.com catalog for agent and map names, and downloads of valreview and vrfkit releases from GitHub.

## Reporting bugs

File a [GitHub issue](https://github.com/Deaadman/valreview/issues). Include the output of `valreview status`.

## Development

Building from source needs current stable Rust with the MSVC toolchain (the Visual Studio "Desktop development with C++" workload).

```powershell
cargo build --release
cargo test
```

- `crates/core`: export loader, analysis and packet rendering.
- `crates/cli`: the `valreview` binary and the interactive shell.

`crates/core/tests/fixtures.rs` runs the whole pipeline on the public replays in `tests/fixtures/public` and needs vrfkit (`VALREVIEW_VRFKIT=<path>` or on PATH). The oracle test needs a personal export at `VALREVIEW_ORACLE_EXPORT` or `tests/data/oracle_export`. Both skip without their inputs.
