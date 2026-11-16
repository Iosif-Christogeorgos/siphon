# Working on Siphon

Siphon is a Tauri 2 desktop video downloader built on yt-dlp and ffmpeg. Architecture is in [README.md](README.md); reasons and known issues are in [docs/decisions.md](docs/decisions.md).

## Handoff rule

**When the conversation reaches about 350K tokens of context, stop and hand off**, without waiting to be asked. If you can't tell how much context is used, hand off early instead of late.

A handoff means, in this order:

1. Finish the small thing in hand, or stop and note it if it is large or uncertain. Start nothing new.
2. Leave the code compiling and passing tests. Commit unverified work as `wip:`, or revert it, whichever leaves the repo cleaner.
3. Commit and push. Tag a version only if its phase is really complete.
4. Write `HANDOFF.md` in the repo root with everything a fresh session needs: what is done, what is in progress and exactly where it stopped, the next steps in order, decisions made and why, known issues with findings so far, and how to run and test.
5. Commit it as `docs: add handoff notes`, push, and give a short summary.

Do the same whenever the owner asks for a handoff or says they are near their usage limit.

## Starting a session

If `HANDOFF.md` exists, read it first, together with the README and the recent git log. Once caught up, fold its lasting parts into `docs/decisions.md` and delete it so it doesn't go stale.

## Workflow

- Build in phases; tag each finished phase (`v0.5.0`, …) and push after it.
- Conventional Commits, one small commit per finished step, each leaving the app working.
- Verify changes in the running app, not only by compiling.
- Never commit secrets, signing keys or downloaded media.
- Releases are created as drafts; the owner publishes them.
