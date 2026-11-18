# Signing in to sites

Some videos are only for signed-in viewers: age-restricted YouTube videos, members-only videos, posts on Instagram or X that need an account. Siphon can download them with the user's own account. It signs in as that user and unlocks only what that account can already watch; it doesn't bypass anything.

Siphon knows nothing about how any site logs in. yt-dlp does, for every site it supports. All Siphon does is give each download the user's cookies for that download's own site.

## What the user sees

When a lookup or a download fails because the site wants an account, the error card says so in one line ("**youtube.com** needs you to sign in") with **Sign in** and, beside it, the way to sign in, as a facet that unfolds on hover or focus:

| Way | What it does |
| --- | --- |
| **Sign-in window** (suggested) | Opens the site's own sign-in page in a Siphon window. The user signs in as usual, two-factor and all, and presses **Done**. On sites whose session cookie is known (YouTube, Instagram, X, Facebook, TikTok, Twitch, Reddit) the window finishes by itself. |
| **A browser** (each one found on this computer) | Takes the sign-in over from that browser, through yt-dlp's `--cookies-from-browser`. Firefox is suggested when it's installed, because it is the browser whose cookies can reliably be read. |
| **cookies.txt** | A Netscape-format cookies file the user exported with a browser extension, picked in a file dialog. |

Whichever way, Siphon runs the lookup that failed again with the new cookies, and only says "Signed in" if it gets past the sign-in: the video loads, or fails with an error of its own (removed, unreadable) instead of "sign in". In the second case the sign-in works and the video's own error shows next; a failed connection or a bot check proves nothing and doesn't count (`sign_in::past_the_wall`). Then it looks the video up again and retries every failed download of that site; downloads still waiting in the queue use the sign-in when their turn comes.

A way that doesn't work says why in a line ("Brave's cookies can't be read"), with the explanation behind the ⓘ, and the choice moves on to the next way worth trying. In the download list, a failed row of such a site has a **Sign in** button that uses the suggested way.

The first sign-in to YouTube shows a short tip: downloading a lot with an account can get it flagged, so a second account is safer.

**Settings → Signed-in sites** lists each site, whether its sign-in still works and when it was last used, how it was made behind the ⓘ, and **Sign out**.

## Which errors offer it

Only errors an account can fix (`ErrorKind::LoginRequired`, `src-tauri/src/ytdlp/errors.rs`): age gates, members-only videos, posts for logged-in viewers, and HTTP 401. yt-dlp ends each of these with a hint to pass cookies or credentials, which is the surest sign.

Not offered, because signing in doesn't help: private videos, videos that must be bought or need a paid membership (YouTube Premium), password-protected videos, removed or region-blocked videos, DRM, and YouTube's bot check. These messages often mention signing in or cookies too, so their rules come first. The bot check is left out on purpose: an account is what YouTube flags there.

## How it works

The code is in `src-tauri/src/sessions/`.

- **Sites** (`site.rs`). A site is the domain anyone can register, from the public suffix list (`youtube.com` for `music.youtube.com`, `bbc.co.uk` for `www.bbc.co.uk`, `alice.github.io` on its own), plus the few sites known by more than one name (`youtu.be` → `youtube.com`, `twitter.com` ↔ `x.com`). An IP address or a single-word host is its own site.
- **Scoping.** A site's cookies are the ones for its domains and their subdomains (`cookies::belongs_to`), never a look-alike such as `notyoutube.com`. YouTube's are `youtube.com` only: yt-dlp signs YouTube's requests with those, and the `google.com` cookies are the whole Google account's.
- **Storage** (`mod.rs`, `dpapi.rs`). One jar per site in `%LOCALAPPDATA%\app.siphon.desktop\sessions\jars\<site>.bin`: the Netscape text, encrypted with DPAPI for the current Windows user (with Siphon's own entropy), written and renamed so a crash can't leave half a file. `index.json` beside it holds which sites are signed in, how, when, when last used and whether the session ran out, and never a cookie.
- **Each yt-dlp run** (lookup, download, playlist estimate, preview copy) of a signed-in site gets a temporary `cookies.txt` in `sessions\temp\` with that site's cookies only (`Sessions::cookie_file`). For a download it belongs to the job's record, like its proxy listener, so it goes however the job ends: finished, failed, paused, cancelled, or the app quitting (`shut_down`). The folder is emptied at launch and on quit as well.
- **Renewed cookies.** yt-dlp writes its cookie jar back to the file when it's done. When the file goes, Siphon compares it with what it wrote and applies only the differences to the stored jar (`cookies::changes`, `cookies::apply`), so two downloads that each renewed a different cookie keep both. Cookies of other domains yt-dlp picked up on the way are dropped.
- **Expiry.** An error that offers sign-in, from a run that had the site's cookies, means the session ran out: the site is marked expired (Settings shows it) and the card says "Your youtube.com sign-in has run out" with **Sign in again**.
- **Never in logs.** Cookies only reach yt-dlp through the file, never the command line. Error details are passed through `cookies::redact`, which hides `Cookie:` and `Set-Cookie:` values and the values in `cookies.txt` lines.

### The sign-in window

`sign_in.rs`. One window, two webviews, which needs Tauri's `unstable` feature:

- At the top, Siphon's own bar (`index.html#sign-in`, `src/features/sign-in/SignInBar.tsx`): the site's name, what to do, ⓘ, **Done** and close. It has the `sign-in` capability.
- Under it, the site's page, with a WebView2 profile of its own per site (`sessions\profiles\<site>\`), apart from Siphon's interface and from other sites. No capability names it and it isn't Siphon's origin, so it can't reach Siphon. It doesn't go through the speed-limit proxy.

**Done** reads the profile's cookies through WebView2's cookie manager (HttpOnly ones included), off the main thread (reading them on it deadlocks), keeps the site's own and checks them. For sites whose session cookie is known, a watcher looks every 1.5 s and checks by itself once it appears, but stays silent if the check fails. Closing the window cancels; closing Siphon's main window closes it too.

Tauri's cookie type drops the leading dot that tells a domain cookie from a host-only one, so every cookie from the window is written as a domain cookie. That can't widen it past its own site.

**Google and embedded sign-in.** Google sometimes refuses to sign in inside an app's browser window ("This browser or app may not be secure"). Siphon doesn't try to get around that check (no changed user agent or the like). It watches the window's navigation, and when Google sends it to its refusal page (`accounts.google.com/…/rejected`, or `disallowed_useragent`), it closes the window and says "Google won't sign in here", suggesting Firefox or a cookies.txt file. In testing (2026-11-18) Google let the owner sign in to YouTube in the window.

### Taking over a browser's sign-in

`browsers.rs`. A browser is offered when its cookie folder exists (`%APPDATA%\Mozilla\Firefox\Profiles`, `%LOCALAPPDATA%\Google\Chrome\User Data`, and so on for Edge, Brave, Opera, Vivaldi, Chromium and Whale). The lookup that failed runs with `--cookies-from-browser <browser> --cookies <temp file>`; yt-dlp writes every cookie it read to that file, Siphon keeps the site's own and deletes the file at once. yt-dlp's errors are turned into plain ones:

| yt-dlp says | Siphon says |
| --- | --- |
| `Could not copy Chrome cookie database` | Close Chrome first: it locks its cookies while it's open. |
| `Failed to decrypt with DPAPI` (or cookies that "could not be decrypted") | Chrome's cookies can't be read: Chromium browsers on Windows encrypt them so only the browser can read them ("app-bound encryption"). Use the sign-in window, Firefox or a cookies.txt file. |
| `could not find … cookies database` | The browser has no cookies: sign in to the site in it first. |
| The site still wants a sign-in | The browser isn't signed in to the site. |

Anything else (a network error) is shown as the error it is. The lookup has the usual 90-second limit, so nothing hangs.

### A cookies.txt file

The dialog opens from Rust (`tauri-plugin-dialog`), so the webview can't make Siphon read any other file. The file must be under 20 MB and hold at least one Netscape cookie line; lines that aren't are skipped. Exporters' variations are accepted: `#HttpOnly_` lines, a missing value, an expiry with decimals, Windows line endings. A file with no cookies at all is "not a cookies.txt file"; one with none for the site says so; one whose cookies don't get through says they may have run out.

## Commands

`sign_in_browsers`, `sign_in` (`url`, `method`: `{type: "window"}`, `{type: "browser", browser}` or `{type: "file"}`; returns `signed_in` with the site and whether to show the tip, `refused` with a line and a reason, or `cancelled`), `signed_in_sites`, `sign_out`, and for the window's bar `sign_in_site`, `sign_in_done` and `sign_in_cancel`. The bar hears `sign-in://status` (`waiting`, `checking`, `not_yet`, `failed`).

## Testing

Unit tests cover the Netscape format both ways, scoping (no cookie reaches another site), DPAPI (round trip, tampered data refused), the error rules with real messages from YouTube, Instagram, X, TikTok, Facebook and Vimeo, expiry, write-back of renewed cookies (including two downloads at once), the cookie file going with the job however it ends, the browser error mapping, and the sign-in page and Google refusal URLs.

**A local site that needs a session.** A small HTTP server with three paths does for every flow without a real account: `/clip.mp4` answers 401 without a valid `session` cookie, `/login` sets one and redirects, `/expire` invalidates every session handed out. yt-dlp's generic extractor downloads `http://127.0.0.1:<port>/clip.mp4`, and its 401 offers sign-in for the site `127.0.0.1`. To simulate signing in inside the window without touching the page, the 401 page can follow its own sign-in link after a second (`<meta http-equiv="refresh" content="1;url=/login">`). The server's log, with whether each request carried a valid session, shows what each check and download sent.

The site's page in the sign-in window runs in its own WebView2 environment, so with `--remote-debugging-port` it can't open the same port and isn't reachable over the DevTools protocol; the bar is. The cookies.txt dialog is a Windows file dialog: set the path with `WM_SETTEXT` on its `ComboBoxEx32 › ComboBox › Edit` and send it `WM_COMMAND IDOK`.

Live results (2026-11-18): the sign-in window with YouTube (Google let the owner sign in; an age-restricted video, `Tq92D6wQ1mg`, then downloaded in full at 1080p) and with Instagram (the window finished by itself once the owner had signed in; see Known issues for what yt-dlp then did with Instagram's posts), the local site end to end (sign in, download, expiry, sign in again with the failed download retried by itself, sign out), the three kinds of cookies.txt, and the two browsers on the test PC: Edge and Brave both refused in about 1.3 s with "cookies can't be read" (yt-dlp: "Failed to decrypt with DPAPI"), even with Brave running. There was no Firefox to test.

## Known issues

- **yt-dlp's signed-in Instagram lookups fail with "HTTP Error 400: Bad Request"** (seen 2026-11-18, reported by others since October 2026). With a working Instagram session, a public post loads, but posts that need the account fail with `Video info extraction failed: HTTP Error 400` in yt-dlp's logged-in request, whatever optional cookies are left out. It's between yt-dlp and Instagram; Siphon picks up yt-dlp's fix by itself when there is one (Nightly sometimes sooner). Before `past_the_wall`, this also meant the sign-in itself could never finish: Done kept failing on the video, though the sign-in had worked.
- See also [decisions.md](decisions.md#known-issues).
