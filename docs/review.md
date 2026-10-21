# Reviewing a match

Type `/review` to open the debrief for your newest match. To pick another one, type `/review --match` and choose it from the list.

The review has six tabs. Move between them with ← and →, or press 1 to 6. Esc closes it.

The pictures on this page are drawn by Vantage itself, using made-up matches.

## Debrief

The short version of the match: how it went, the issues that cost you the most rounds, what to keep doing, and how you did on your [focus habit](habits.md#your-focus).

![The Debrief tab](images/review-debrief.svg)

Press Enter to go to the Fixes tab.

## Fixes

Each issue in full: what happened, why it costs you, what to try instead, and what to check next match. Use ↑ and ↓ to pick an issue, and press Enter to see its first death on the map.

![The Fixes tab](images/review-fixes.svg)

## Map

The minimap, drawn in the terminal, with your deaths and your best and worst spots. It's turned so the attackers start at the bottom, the way you see the map on attack. Use ↑ and ↓ to step through your deaths.

![The Map tab](images/review-map.svg)

## Clips

Your clips matched to your deaths, with each clip's length and size. A clip counts when your clip app saved it within 90 seconds of the death.

The first time, Vantage looks for the folders of these apps and lets you pick one:

- Medal
- NVIDIA ShadowPlay
- Xbox Game Bar
- AMD Adrenalin
- Outplayed
- SteelSeries Moments

You can also type your own folder. Change it later in `/settings`.

![The Clips tab](images/review-clips.svg)

## Screenshots

A screenshot of the seconds before each death, taken from its clip. They're saved in `Pictures\Vantage\Clips`. Press `o` to open the folder.

Screenshots need ffmpeg. The installer gets it for you. If it's missing, the bottom of the screen says so, and `/update` installs it.

![The Screenshots tab](images/review-screenshots.svg)

## AI

Optional. An AI looks at the screenshots for things the replay can't show, like where you stood and where your crosshair was.

You can use:

- Claude, ChatGPT or Gemini, with your own API key
- Ollama, which runs on your PC and sends nothing anywhere

Pick the AI and its model in `/settings`. Then press **Ctrl+Enter** on this tab to run the review. Each match is reviewed once, and the answer is saved.

![The AI tab](images/review-ai.svg)

[What gets sent to the AI](privacy.md#ai-review)
