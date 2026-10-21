# Vantage

[![Latest release](https://img.shields.io/github/v/release/Deaadman/vantage?style=flat-square&label=release)](https://github.com/Deaadman/vantage/releases/latest) ![Windows x64](https://img.shields.io/badge/Windows-x64-blue?style=flat-square) [![License](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)

Vantage is a VALORANT coach that reads your replays. After each match it shows why you died and the three things that cost you the most rounds. Over time, it tracks the mistakes you keep making.

It runs on your PC and never touches the game or the Riot client.

![A match debrief in Vantage](docs/images/review-debrief.svg)

## Install

Vantage runs on 64-bit Windows. Open PowerShell and run:

```powershell
irm https://raw.githubusercontent.com/Deaadman/vantage/main/install.ps1 | iex
```

Then:

1. In VALORANT, download the replays of the matches you want reviewed.
2. Open a new terminal and type `vantage`.
3. Type `/settings` and set your rank. Replays don't include it.

That's it. Vantage reads your replays when it opens and picks up new ones while it's open. It updates itself.

## What it does

| Command | What you get |
| --- | --- |
| `/review` | What went wrong in a match, and what to try next time |
| `/habits` | The mistakes you keep making, and whether they're getting better |
| `/focus` | One habit to work on, checked in every debrief |
| `/plan` | A game plan for a map: where you win, where you lose |
| `/rank` | What wins games at your rank |

Type `/` in Vantage to see every command, or `/help` for the keys.

## Learn more

- [Reviewing a match](docs/review.md): the six tabs, clips, screenshots and AI review
- [Habits and focus](docs/habits.md): the 21 habits Vantage tracks
- [Map plans and your rank](docs/plan-and-rank.md)
- [Commands and keys](docs/commands.md)
- [Settings and updates](docs/settings.md)
- [Your data](docs/privacy.md): what's stored, and what goes over the internet
- [Building from source](docs/development.md)

## Bugs and ideas

Found a bug? Use the [bug report form](https://github.com/Deaadman/vantage/issues/new?template=bug_report.yml). Type `/logs` in Vantage to find the log it asks for.

Have an idea? Use the [feature request form](https://github.com/Deaadman/vantage/issues/new?template=feature_request.yml).

## Credits

Replays are read by [vrfkit](https://github.com/yakisoba0728/vrfkit), which the installer sets up for you. Vantage is licensed under [GPL-3.0](LICENSE).
