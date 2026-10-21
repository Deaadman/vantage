# Your data

Vantage runs on your PC. It reads your replay files and never touches the game or the Riot client.

## Where Vantage is installed

The installer puts Vantage, and the vrfkit version it needs, in `%LOCALAPPDATA%\vantage`, and adds it to your PATH so `vantage` works in any terminal. It checks both downloads against their SHA-256 before installing them.

## What's stored, and where

Everything is in `%LOCALAPPDATA%\vantage\data`:

- `config.toml`: your settings, and your AI API key if you add one
- a database of what happened in each match
- movement tracks of each match, 4 times a second
- session logs, for bug reports

Screenshots are saved in `Pictures\Vantage\Clips`.

Vantage reads each replay with [vrfkit](https://github.com/yakisoba0728/vrfkit), then deletes vrfkit's raw output once it's done.

To keep your data somewhere else, start Vantage with `--data-dir <folder>`, or set the environment variable `VANTAGE_DATA_DIR`.

To delete Vantage and everything above except your screenshots, close Vantage and run `vantage uninstall`. It deletes only the files Vantage made, even in a folder you chose yourself.

## What goes over the internet

- Agent, map and weapon names, and the minimap pictures, from [valorant-api.com](https://valorant-api.com), once a day
- New versions of Vantage and vrfkit, from GitHub, when Vantage opens and every few hours while it stays open
- ffmpeg, from winget, if it's missing

Nothing about your matches is sent anywhere, unless you turn on AI review.

## AI review

If you use Claude, ChatGPT or Gemini:

- Choosing a model asks that AI for its list of models.
- When you run a review, the facts of that match and its screenshots are sent to that AI.

Nothing is sent until you press Ctrl+Enter on the AI tab, and only for that one match.

Ollama runs on your PC, so with Ollama nothing leaves your computer.

## Session logs

Each run writes a short log: the version, which commands ran, and any warnings, errors or crashes. Logs never contain match data, and your home folder and Windows user name are removed. Vantage keeps the last 20. Type `/logs` to open the folder.
