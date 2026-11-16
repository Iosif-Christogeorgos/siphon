# Handoff

Written 2026-11-17, at the end of a session that made downloads faster and more stable. The owner was near their usage limit, so the session stopped before the release was finished.

The work is meant to ship as **v0.5.2**. It is not tagged and the version is not bumped, because two things below are still open.

## What is done

Five commits on `main`, on top of `v0.5.1`. Each was checked in the running app unless noted.

| Commit | Change | Measured in the app |
| --- | --- | --- |
| `a16a683` | Trimmed parts are fetched by ffmpeg in the request size the site asks for (`-request_size`, 10 MB on YouTube). | A five-minute 1080p part: 172 s before, about 6 s after. An exact cut still lands on its frames. |
| `86e3de0` | Segmented streams (HLS, DASH) download four fragments at once. | 255 MiB in 5 s (about 51 MiB/s), up from 23 MiB/s. |
| `634056a` | A retry carries on from the partial file. A failed download keeps a record of its files, as a paused one did. Removing a failed download deletes them. | Automatic retry, **Try again** and pause/resume all continued with a `Range` request and gave a file with the right hash. |
| `cc633f3` | Errors say whether they may pass. A dropped or refused connection (including HTTP 403) gets two more tries after 2 and 10 s; HTTP 429 gets one after 30 s. yt-dlp's own retries are five with backoff instead of ten with none. A missing fragment fails the try instead of leaving a hole. New stage "Trying again in a moment". | A 5 s outage was absorbed inside one try. A 30 s outage recovered and resumed. Cancelling during the wait was instant and removed the partial file. |
| `aa4a1d0` | An "outdated" failure makes the next try look the video up again instead of trusting the session's cache. | **Not exercised in the app**: I found no way to make the cache go stale on demand. Compiles, tests pass. |

`cargo test` passes (53 tests) and `npm run typecheck` is clean.

## What is in progress

**Removing partial files when the app closes.** It was written, failed its test, and was reverted, so nothing of it is in the repository.

The idea: the download list is not kept between sessions, and a new download ignores old partial files, so whatever a paused, failed or running download leaves at exit is never used again. The change replaced `DownloadManager::pause_all` with a `shut_down` that:

1. stops every job with `Stop::Cancel` (each job then removes its own files),
2. waits up to 3 s for `is_idle()`,
3. drains the `leftovers` map and removes those files too,

and called it from the `RunEvent::Exit` handler in `src-tauri/src/main.rs`.

The test: one paused download and one running, then `taskkill /IM siphon.exe` (no `/F`). Result: `siphon.exe` was still alive 15 s later and both `.part` files were still there. I did not find out why. Two things to check first:

- whether `taskkill` without `/F` reaches `RunEvent::Exit` at all in a dev build. Closing the window by hand, or through the title bar's Close button over the DevTools protocol, is the better test. The existing `pause_all` was never tested this way either, so it may have the same problem.
- whether `shut_down` hangs. `block_on` runs on the main thread while the jobs finish on the runtime's worker threads; that should work, but it is unproven.

## Next steps, in order

1. Finish or drop the exit cleanup above.
2. Update the docs, which still describe 0.5.1:
   - `README.md`: "tried once more on its own" under Queue; "Closing the app pauses whatever is running" (only if step 1 lands); "Update and retry" under Updates.
   - `docs/decisions.md`: "A network error gets one automatic retry"; "Pausing kills the process…" (retries now reuse the same mechanism); the known issue about HTTP 403 on section downloads; add the decisions listed below.
3. Run the regression pass in the app with the test links in `docs/decisions.md`: a plain YouTube download, a playlist with three at once, a preset that converts, pause and resume during a conversion.
4. Set the version to 0.5.2 in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit, tag `v0.5.2`, push. The tag builds a draft release; the owner publishes it.
5. Fold this file into `docs/decisions.md` and delete it.

## Decisions made, and why

- **The request size comes from the site, not from Siphon.** yt-dlp's metadata names it per format (`downloader_options.http_chunk_size`). Sites that don't name one are left alone, because a server that ignores `Range` could misbehave.
- **`-request_size` is only passed after checking ffmpeg knows it** (`ffmpeg -h protocol=http`, cached in `state::ChunkedRequests`). An older ffmpeg stops with "Unrecognized option". This matters on macOS, where ffmpeg comes from Homebrew.
- **Four fragments, not eight.** Eight measured no faster, and three downloads at once already means twelve connections to one site.
- **The backend decides whether a try continues, not the frontend.** The `resume` flag is gone from `DownloadRequest`. A try continues when the `leftovers` map has an entry for the download's id; otherwise it starts fresh with `--force-overwrites`.
- **A download that began by replacing an existing file starts over on every try** (`Leftovers::replaces_existing`). Without `--force-overwrites` yt-dlp would take the old file for the result and report success. This showed up in testing.
- **Stream files carry their own "intermediate" flag** instead of one flag per download, so a retry that picks different streams can't delete a finished file. A trimmed part is never an intermediate: it arrives under its final name.
- **`Retry` lives on `AppError` but is not sent to the frontend** (`#[serde(skip)]`), so no frontend type changed for it. HTTP 429 keeps the kind `Outdated`, so the yt-dlp update check still runs first.
- **Being offline now takes about 15 s to report instead of about 5**, the price of the second retry. A drop in the middle of a download takes about 80 s to give up.
- **Not done on purpose:** reusing the lookup's JSON for the download (`--load-info-json`). It would save 1.2 s per download and risks expired links.

## Known issues and findings

- **A stalled connection during a trimmed download may hang.** ffmpeg has no read timeout by default and yt-dlp passes none. `-rw_timeout` in the `ffmpeg_i` arguments would be the fix. Untested, so not added.
- **An update can't be swapped in while other downloads run**, so "update and retry" does nothing for a playlist where every video fails for the same reason. This follows from the decision in `docs/decisions.md` not to make the swap wait.
- **A conversion that fails deletes its downloaded source**, so **Try again** downloads it again.
- **HLS trimmed parts print many "keepalive request failed" warnings** from ffmpeg. They are harmless; the download works.

## How to run and test

```sh
npm install
npm run tauri dev
cargo test --manifest-path src-tauri/Cargo.toml
npm run typecheck
```

To drive the running app without touching the desktop, see the testing notes in `docs/decisions.md` (remote debugging on port 9222, `window.__TAURI_INTERNALS__.invoke`). Download events can be collected with:

```js
const T = window.__TAURI_INTERNALS__;
const handler = T.transformCallback((event) => console.log(event.payload));
await T.invoke("plugin:event|listen", { event: "download://event", target: { kind: "Any" }, handler });
```

To test retries, serve a file from a small local HTTP server that supports `Range`, cuts the connection after N bytes and then refuses requests for a set time. yt-dlp's generic extractor downloads `http://127.0.0.1:<port>/clip.mp4` like any other link. Point the download folder at a scratch folder first, and set it back afterwards.
