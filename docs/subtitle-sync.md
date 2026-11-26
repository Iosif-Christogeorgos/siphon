# Subtitle sync

A subtitle file added to a film can be synced to the film's speech: **Sync to speech**, in the subtitle track's details in the tracks card, gives every line the moment its words are spoken. It works line by line, not by moving the whole file, so a file whose lines are off in different directions (one 1.5 s late, the next 2 s early) comes out right, and it needs no other subtitles to compare with: the film's own sound is the reference.

It is an optional download (Settings › Subtitle sync), made only when the user asks for it. Until then the button is locked and points there. Once downloaded it runs offline: the models are loaded from disk (`HF_HUB_OFFLINE`), and nothing leaves the PC.

## What it does

`src-tauri/sync/siphon_sync.py`, run by `src-tauri/src/subsync/` in a private Python:

1. **The dialogue** is pulled out of the chosen sound track by Siphon's FFmpeg: the centre channel of a 5.1 or 7.1 mix, where films put speech, away from most music; both channels of stereo. 16 kHz mono.
2. **Where someone speaks**: Silero VAD (MIT). It runs over 32 stretches of the film at once, each first hearing the 60 s before it so its memory has settled as if it had heard everything before; its own rules then turn its probabilities into speech regions (from 0.35, silence below 0.20 for 250 ms, 150 ms margins). Measured on Sicario: 1,081 of 1,082 regions the same as one long pass, in 11 s instead of 48.
3. **What is said, word by word**: Qwen3-ASR 1.7B (Apache-2.0) writes it down, chunks of up to 30 s cut in silences, 16 at a time; Qwen3-ForcedAligner 0.6B (Apache-2.0) times every word. Its published accuracy is 25-45 ms on average against about 130 ms for WhisperX's method. A scene in another language is written down in that language (Sicario's Spanish was).
4. **Phrases**: the words cut at pauses of 0.3 s and at punctuation the recogniser wrote.
5. **A rough whole-film fit** places each line near its speech first: lines on screen against speech, every 10 ms, by FFT cross-correlation, over seven frame-rate ratios (25 / 23.976 and the like), then a straight line through the offsets of 10-minute stretches for a slow drift. No line is paired with speech more than 7 s from where this puts it.
6. **Pairing by meaning**: BGE-M3 (MIT) compares every line, and every run of 2-3 lines, with every nearby run of phrases, across languages (Greek lines against English speech). One pass of dynamic programming picks the pairing that agrees best overall, in order. A line may take several phrases; lines may share one phrase, which is then cut where each part best means what its line says; speech no line takes (an untranslated "Hmm") and lines no speech matches (a sign, a credit) stay unpaired.
7. **Times**: a paired line starts 0.04 s before its first word and stays 0.45 s after its last, lengthened to be readable (20 characters a second) without running into the next line. A line too quick to read at all may start up to 0.25 s early, taken from the line before.
8. **What pairing missed**: lines and speech both left unpaired between the same two paired lines are each other (a name the recogniser misheard: "Wan" heard as "One"). Words the aligner stacked on one instant are re-timed from the words around them, and a line whose first word the recogniser missed starts where the voice detector heard speech begin. Anything still unplaced takes the shift of the lines around it.

The synced copy is written as UTF-8 SRT in `subsync\work\` and takes the added file's place in the draft (`withSynced` in `draft.ts`): every choice made of its track (language, name, flags, place) is kept, the row says **Synced**, and **Use the file's own timing** puts the file back. The copy then takes it as it would any file added.

## The download

`<app-local-data>\subsync\`, nothing outside it:

| Part | Download | On disk |
|---|---|---|
| uv 0.13.10 (Astral, checked against its published SHA-256) | 22 MB | 50 MB |
| Python 3.14, private: `--no-registry --no-bin`, uv told to use only its own (`UV_PYTHON_PREFERENCE=only-managed`), so the user's own Python is neither used nor touched | 30 MB | 130 MB |
| PyTorch 2.11 with CUDA 12.8, and every other package at the version tested (`sync/requirements.txt`, from `uv pip freeze`) | 3.3 GB | 4.9 GB |
| Qwen3-ASR 1.7B, Qwen3-ForcedAligner 0.6B, BGE-M3 at pinned revisions; BGE-M3 stored at half precision, which is how it runs | 8.8 GB | 7.3 GB |

uv's download cache is deleted once the packages are copied out of it. A download that stops carries on where it stopped (each model has a `.complete` mark); `ready.json`, written last, says the whole is there, and its `VERSION` makes an older install be made again when the packages or models change. **Remove** in Settings deletes the folder.

The download is offered only where `nvidia-smi` lists an NVIDIA card with 8 GB or more; the models run in bfloat16 on CUDA.

## Measured

On the owner's PC (RTX 5080, 16 GB), 2026-11-26.

- **Sicario** (2 h 1 min, 7.1 English with Spanish scenes; a Greek SRT from another release that is about 1 s late at the start and 2 s at the end, and off line by line). The owner reported seven lines by ear (9:21, 10:23-10:26, 11:50, 12:09, 12:17, 35:15, 59:57); all came out where they were heard, and the owner judged the whole file right. 903 lines: 868 paired by their words, 35 placed between them. **1 min 46 s** in all, with the models loaded.
- **Korra S02E08** (24 min, a correctly timed Greek SRT scrambled on purpose: a 1.7 s delay, a slow drift, and every line moved by up to ±2 s): from a median 2.1 s off to **0.12 s**; 84% of lines within 0.2 s and 89% within 0.5 s of the original timing, in 27 s. Of the 11 lines still a second or more apart, several are the original's own errors (it is fan-made: "Ράβα!" is spoken at 13:50.56, synced to 13:50.52, the original says 13:52.89) and five are credits that are never spoken.
- Speeding it up was checked against the version the owner approved: every one of the 903 lines within 0.1 s of it.

## Limits

- **SRT files added to a film**, for now. ASS keeps styling that would have to be rewritten; a subtitle track already inside the film would have to be taken out first.
- **Speech in 11 languages** is timed (Qwen3-ForcedAligner's: English, Chinese, Cantonese, French, German, Italian, Japanese, Korean, Portuguese, Russian, Spanish). A sound track tagged with another (Greek) is refused with that said. The subtitles may be in any language BGE-M3 reads.
- **One file at a time**, from the file's own card; a season's episodes one by one.
- Lines nobody speaks (signs, credits) can only be placed by their neighbours.
- **Tested on one card.** The setting of 8 GB comes from what the models use (about 7 GB at their peak), not from a smaller card.

## Choices

- **Speech recognition, not subtitle matching.** Most films the owner syncs have no other subtitles, and matching lines against another language's subtitles by timing alone could not tell which line was which where the two break dialogue differently (tried 2026-11-26: every fix for one spot broke another).
- **Nothing copied from other projects.** sc0ty's subsync (GPL, fits one straight line through all of a file, so it cannot fix lines off in different directions) and Anchor (AGPL: WhisperX and NLLB translation; it got 2 of the owner's 7 spots right on Sicario, its misses being lines it could not match by word and guessed) were studied; the pipeline here is written from scratch.
- **Permissive models only**: Apache-2.0 and MIT. NLLB, which Anchor uses, is CC-BY-NC.
- **The largest models the card runs easily** rather than the fastest: Qwen3-ASR 1.7B over 0.6B, the aligner over Whisper's own word times. NVIDIA's Nemotron 3.5 Transcribe (30B) was too large for 16 GB.
- **A private Python rather than the user's.** Other users won't all have one, and one the user upgrades or removes would break the sync.

## Testing

Drive the dev app over the DevTools protocol (see [decisions.md](decisions.md#testing-notes)). `subsync_status`, `subsync_install` and `subsync_remove` can be invoked directly; the dev build's download is `%LOCALAPPDATA%\app.siphon.desktop.dev\subsync\`. To check timing without watching, scramble a correctly timed SRT (a delay, a drift, each line moved by up to ±2 s) and compare each line's start with the original's. Work on hard links or copies of media in a scratch folder: the copy's `Edited` folder is made beside the film.
