# Commands

Run `vantage` in a terminal to open the shell. When it opens, it reads any new replays and works out which account is yours. While it's open, it picks up each replay you download.

![Vantage when it opens](images/shell-ready.svg)

Type `/` to see the commands. Keep typing to filter them, and press Tab to complete one.

![The command menu](images/shell-commands.svg)

| Command | What it does |
| --- | --- |
| `/review` | What went wrong in a match, and what to fix next time. See [Reviewing a match](review.md). |
| `/habits` | The mistakes you keep making, and which are getting better. See [Habits and focus](habits.md). |
| `/focus` | Pick one habit to work on. |
| `/plan` | A game plan for one map. See [Map plans and your rank](plan-and-rank.md). |
| `/rank` | What wins games at your rank, and how you compare. |
| `/matches` | Your matches, newest first, with each one's mode. Add `--all` for more than 10. Add a mode to see only that mode, e.g. `/matches swiftplay` or `/matches comp`. Competitive, Unrated and Premier share the label Comp/Unrated/Premier, because a replay doesn't record its queue, so `comp`, `unrated` and `premier` all show those matches. Matches stored before Vantage read modes have none, so a mode filter leaves them out. |
| `/scan` | Look for new replays now. Add `--retry-failed` to try failed replays again. |
| `/settings` | Your rank, folders, AI review and other settings. See [Settings and updates](settings.md). |
| `/logs` | Open the folder of session logs, to attach one to a bug report. |
| `/update` | Install the newest Vantage and vrfkit now. |
| `/release-notes` | What's new in each version: pick one, or show them all. |
| `/tui` | `/tui fullscreen` (the default) fills the terminal. `/tui default` keeps the output in the terminal after you quit. Vantage remembers your choice. |
| `/help` | The commands and keys. |
| `/clear` | Clear the screen. |
| `/exit` | Quit Vantage. |

Add `--help` to a command to see all its options, for example `/scan --help`.

## Keys

| Key | What it does |
| --- | --- |
| Enter | Run the line, or the highlighted command |
| Tab or → | Complete the highlighted suggestion |
| ↑ ↓ | Move in the menu, or go back through what you typed |
| Esc | Hide the menu, or close a panel |
| PgUp PgDn | Scroll the output (with `/tui default`, scroll the terminal instead) |
| Ctrl+L | Clear the screen |
| Ctrl+C | Clear the line. Press it twice on an empty line to quit. |

## Outside the shell

Every command also works straight from the terminal, without the `/`. For example:

```powershell
vantage review
vantage plan --map Lotus
vantage matches --all
vantage matches swiftplay
```

`vantage review` and `vantage plan` print as text, so you can save or share them.

`vantage mcp` is for your AI app, not for typing: the app runs it to review your matches. See [With your AI app](review.md#with-your-ai-app).
