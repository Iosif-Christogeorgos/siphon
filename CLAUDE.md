# Working on Siphon

Siphon is a Tauri 2 desktop video downloader built on yt-dlp and ffmpeg. Architecture is in [README.md](README.md); reasons and known issues are in [docs/decisions.md](docs/decisions.md).

## Handoff rule

**When the conversation reaches about 350K tokens of context, stop and hand off**, without waiting to be asked. If you can't tell how much context is used, hand off early instead of late.

A handoff means, in this order:

1. Finish the small thing in hand, or stop and note it if it is large or uncertain. Start nothing new.
2. Leave the code compiling and passing tests. Commit unverified work as `wip:`, or revert it, whichever leaves the repo cleaner.
3. Commit and push. Don't tag a release as part of a handoff.
4. Write `HANDOFF.md` in the repo root with everything a fresh session needs: what is done, what is in progress and exactly where it stopped, the next steps in order, decisions made and why, known issues with findings so far, and how to run and test.
5. Commit it as `docs: add handoff notes`, push, and give a short summary.

Do the same whenever the owner asks for a handoff or says they are near their usage limit.

## Starting a session

If `HANDOFF.md` exists, read it first, together with the README and the recent git log. Once caught up, fold its lasting parts into `docs/decisions.md` and delete it so it doesn't go stale.

## Workflow

- Work on `main`. Conventional Commits, one small commit per finished step, each leaving the app working and tested, so `main` can be released at any time. Push after each commit or small group of commits.
- Verify changes in the running app, not only by compiling. How to drive it, test retries and test closing is in [docs/decisions.md](docs/decisions.md#testing-notes).
- Never commit secrets, signing keys or downloaded media.

## Releases

- **Don't release every change.** Each release asks users to restart Siphon.
- **Release soon for a fix to something broken:** a download that fails, a wrong or damaged file, a crash.
- **Batch everything else** (looks, wording, small features) into one release every few weeks, or once a few visible improvements have built up. Suggest a release to the owner when that point is reached; don't cut one unasked.
- **Site changes never need a release:** yt-dlp, FFmpeg and Deno update themselves in the background.
- **To release:** set the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` (patch for fixes, minor for new features), commit `chore: bump to x.y.z`, then tag `vx.y.z` and push the tag. The workflow builds a draft release; the owner publishes it.
