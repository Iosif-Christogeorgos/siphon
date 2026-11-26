# Siphon's subtitle sync: gives every line of a subtitle file the moment its words are spoken in a film.
# Siphon runs it in its own private Python (see src-tauri/src/subsync/mod.rs and docs/subtitle-sync.md).
#
#   python siphon_sync.py setup --models DIR
#       Downloads the three models into DIR, at pinned revisions, and stores the meaning model at half size.
#   python siphon_sync.py run --models DIR --video FILM --stream N [--language eng] --subs FILE.srt|.ass
#                             [--charset CP1253] --out OUT.srt|.ass
#       Syncs the subtitles to the speech and writes OUT (UTF-8), SRT as SRT and ASS or SSA as ASS or SSA.
#
# Every line it prints on stdout is one JSON object: {"event": "progress", ...}, then one "done" or "error".
#
# How a sync works:
#   1. The dialogue is pulled out of the film (its centre channel, where films put speech).
#   2. A voice detector (Silero VAD) finds where someone speaks, 32 stretches of the film at once.
#   3. Qwen3-ASR writes down what is said, and Qwen3-ForcedAligner times every word.
#   4. The words are cut into short phrases at pauses and punctuation.
#   5. A rough whole-film fit (speech against lines on screen, by FFT, with frame-rate ratios and a slow
#      drift) says roughly where each line belongs.
#   6. BGE-M3 compares every line, and every run of 2-3 lines, with every nearby run of phrases by meaning,
#      across languages; dynamic programming picks the pairing that agrees best, in order.
#   7. A paired line starts on its first word and ends just after its last, kept readable; lines left
#      unpaired are paired with speech left over between the same neighbours, else placed by them.
import argparse
import json
import os
import re
import subprocess
import sys
import threading
import time

SR = 16000
REVISIONS = {
    "asr": ("Qwen/Qwen3-ASR-1.7B", "7278e1e70fe206f11671096ffdd38061171dd6e5"),
    "aligner": ("Qwen/Qwen3-ForcedAligner-0.6B", "c7cbfc2048c462b0d63a45797104fc9db3ad62b7"),
    "meaning": ("BAAI/bge-m3", "5617a9f61b028005a4858fdac845db406aefb181"),
}
MODELS_VERSION = 1

# ISO 639 codes as Matroska tags them, to the names Qwen3-ASR takes.
LANGUAGES = {
    "en": "English", "eng": "English", "zh": "Chinese", "chi": "Chinese", "zho": "Chinese",
    "yue": "Cantonese", "ar": "Arabic", "ara": "Arabic", "de": "German", "ger": "German", "deu": "German",
    "fr": "French", "fre": "French", "fra": "French", "es": "Spanish", "spa": "Spanish",
    "pt": "Portuguese", "por": "Portuguese", "id": "Indonesian", "ind": "Indonesian",
    "it": "Italian", "ita": "Italian", "ko": "Korean", "kor": "Korean", "ru": "Russian", "rus": "Russian",
    "th": "Thai", "tha": "Thai", "vi": "Vietnamese", "vie": "Vietnamese", "ja": "Japanese", "jpn": "Japanese",
    "tr": "Turkish", "tur": "Turkish", "hi": "Hindi", "hin": "Hindi", "ms": "Malay", "may": "Malay",
    "msa": "Malay", "nl": "Dutch", "dut": "Dutch", "nld": "Dutch", "sv": "Swedish", "swe": "Swedish",
    "da": "Danish", "dan": "Danish", "fi": "Finnish", "fin": "Finnish", "pl": "Polish", "pol": "Polish",
    "cs": "Czech", "cze": "Czech", "ces": "Czech", "fil": "Filipino", "tl": "Filipino", "tgl": "Filipino",
    "fa": "Persian", "per": "Persian", "fas": "Persian", "el": "Greek", "gre": "Greek", "ell": "Greek",
    "hu": "Hungarian", "hun": "Hungarian", "mk": "Macedonian", "mac": "Macedonian", "mkd": "Macedonian",
    "ro": "Romanian", "rum": "Romanian", "ron": "Romanian",
}
# The languages Qwen3-ForcedAligner can time words in.
TIMED = {"Chinese", "English", "Cantonese", "French", "German", "Italian", "Japanese", "Korean", "Portuguese",
         "Russian", "Spanish"}


def say(**event):
    print(json.dumps(event, ensure_ascii=False), flush=True)


def progress(stage, fraction):
    say(event="progress", stage=stage, fraction=round(max(0.0, min(1.0, fraction)), 4))


# ───────────────────────────── setup ─────────────────────────────

def folder_bytes(path):
    total = 0
    for root, _, files in os.walk(path):
        for name in files:
            try:
                total += os.path.getsize(os.path.join(root, name))
            except OSError:
                pass
    return total


def setup(models):
    from huggingface_hub import HfApi, snapshot_download
    os.makedirs(models, exist_ok=True)
    api = HfApi()
    wanted = {
        "asr": None,
        "aligner": None,
        # Only what sentence-transformers needs: not the ONNX copy, the ColBERT and sparse heads or the images.
        "meaning": ["pytorch_model.bin", "config.json", "tokenizer.json", "tokenizer_config.json",
                    "special_tokens_map.json", "sentencepiece.bpe.model", "modules.json",
                    "sentence_bert_config.json", "config_sentence_transformers.json", "1_Pooling/*"],
    }
    plan = []
    for key, (repo, rev) in REVISIONS.items():
        info = api.model_info(repo, revision=rev, files_metadata=True)
        files = [s for s in info.siblings if not s.rfilename.endswith((".md", ".gitattributes"))]
        if wanted[key] is not None:
            import fnmatch
            files = [s for s in files if any(fnmatch.fnmatch(s.rfilename, p) for p in wanted[key])]
        plan.append((key, repo, rev, [s.rfilename for s in files], sum(s.size or 0 for s in files)))
    total = sum(p[4] for p in plan)
    done = 0
    for key, repo, rev, files, size in plan:
        target = os.path.join(models, key if key != "meaning" else "meaning.download")
        if key != "meaning" and os.path.exists(os.path.join(target, ".complete")):
            done += size
            continue
        stop = threading.Event()

        def watch(base=done, where=target):
            while not stop.wait(0.5):
                say(event="progress", stage="models", done=base + folder_bytes(where), total=total)
        watcher = threading.Thread(target=watch, daemon=True)
        watcher.start()
        try:
            snapshot_download(repo, revision=rev, local_dir=target, allow_patterns=files)
        finally:
            stop.set()
            watcher.join()
        done += size
        say(event="progress", stage="models", done=done, total=total)
        if key == "meaning":
            # Stored at half size: the sync runs it at half precision anyway, so nothing changes.
            say(event="progress", stage="meaning", fraction=0.0)
            from sentence_transformers import SentenceTransformer
            model = SentenceTransformer(target, device="cpu")
            model.half()
            final = os.path.join(models, "meaning")
            model.save(final, safe_serialization=True)
            import shutil
            shutil.rmtree(target, ignore_errors=True)
            open(os.path.join(final, ".complete"), "w").close()
        else:
            import shutil
            shutil.rmtree(os.path.join(target, ".cache"), ignore_errors=True)
            open(os.path.join(target, ".complete"), "w").close()
    json.dump({"version": MODELS_VERSION}, open(os.path.join(models, "ready.json"), "w"))
    say(event="done", bytes=folder_bytes(models))


# ───────────────────────────── run ─────────────────────────────

def read_text(path, charset):
    raw = open(path, "rb").read()
    tries = ([charset] if charset else []) + ["utf-8-sig", "cp1253", "cp1252"]
    for enc in tries:
        try:
            return raw.decode(enc)
        except (UnicodeDecodeError, LookupError):
            pass
    return raw.decode("utf-8", "replace")


TIME = re.compile(r"(\d+):(\d+):(\d+)[,.](\d+)")


def sec(t):
    h, m, s, ms = TIME.match(t.strip()).groups()
    return int(h) * 3600 + int(m) * 60 + int(s) + int(ms.ljust(3, "0")[:3]) / 1000


def stamp(s):
    ms = max(0, int(round(s * 1000)))
    return f"{ms // 3600000:02d}:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d},{ms % 1000:03d}"


def read_lines(path, charset):
    blocks = re.split(r"\n\s*\n", read_text(path, charset).replace("\r", "").strip())
    lines = []
    for b in blocks:
        rows = b.split("\n")
        k = next((n for n, r in enumerate(rows) if "-->" in r), None)
        if k is None or not TIME.search(rows[k]):
            continue
        a, z = rows[k].split("-->")
        body = rows[k + 1:]
        plain = " ".join(re.sub(r"<[^>]+>|\{[^}]*\}", "", r).strip().lstrip("-–— ").strip() for r in body).strip()
        lines.append({"s": sec(a), "e": sec(z.split()[0]), "body": body, "text": plain or "…"})
    return lines


# ── ASS (and SSA) ──
# Only the dialogue is paired with speech. Signs (pinned to the picture), songs (karaoke, timed by the
# syllable from the line's start) and anything drawn are moved with the dialogue around them, keeping
# their length; comments are left as they are. Copies of a line on several layers (a glow under it, an
# outline over it) are one line. Only an event's Start and End are rewritten: every other byte stays.

ASS_TIME = re.compile(r"^\s*(\d+):(\d{1,2}):(\d{1,2})[.:](\d{1,3})\s*$")
# Style names that say what isn't dialogue, by word: "Signs", "TS", "OP Romaji", "ED-Kanji", "Title".
NOT_SPOKEN = {"sign", "signs", "ts", "typeset", "typesetting", "title", "titles", "eyecatch", "note", "notes",
              "insert", "logo", "op", "ed", "opening", "ending", "song", "songs", "lyric", "lyrics", "karaoke",
              "kara", "romaji", "kanji", "music"}


def ass_sec(t):
    m = ASS_TIME.match(t)
    h, mi, s, frac = m.groups()
    return int(h) * 3600 + int(mi) * 60 + int(s) + int(frac) / 10 ** len(frac)


def ass_stamp(s, like):
    """`s` seconds as ASS writes a time, with as many decimals as the time it replaces (centiseconds)."""
    places = len(ASS_TIME.match(like).group(4))
    units = max(0, int(round(s * 10 ** places)))
    whole, frac = divmod(units, 10 ** places)
    return f"{whole // 3600}:{whole // 60 % 60:02d}:{whole % 60:02d}.{frac:0{places}d}"


def ass_plain(text):
    plain = re.sub(r"\{[^}]*\}", "", text)
    plain = re.sub(r"\\[Nnh]", " ", plain)
    return re.sub(r"\s+", " ", plain).strip().lstrip("-–— ").strip()


def ass_kind(event):
    """dialogue, moved (signs, songs, drawings, an effect) or kept (comments)."""
    if event["kind"] == "comment":
        return "kept"
    text, style, effect = event["text"], event["style"], event["effect"]
    if re.search(r"\\(?:k|K|kf|ko)\d", text):                          # karaoke syllables
        return "moved"
    if re.search(r"\\(?:pos|move|org|i?clip)\s*\(|\\p[1-9]", text):     # pinned, moving or drawn
        return "moved"
    if effect.strip():                                                  # Banner;, Scroll up; …
        return "moved"
    if set(re.findall(r"[a-z]+", style.lower())) & NOT_SPOKEN:          # "Signs", "OP1 Romaji", "ED-Kanji"
        return "moved"
    if not ass_plain(text):
        return "moved"
    return "dialogue"


def read_ass(path, charset):
    """The file's events, if it is ASS or SSA; None if it isn't."""
    raw = open(path, "rb").read()
    text = read_text(path, charset)
    if not re.search(r"^\s*\[Events\]\s*$", text, re.M | re.I):
        return None
    rows = text.split("\n")
    events, section, fields = [], None, None
    for n, row in enumerate(rows):
        line = row[:-1] if row.endswith("\r") else row
        bare = line.strip().lstrip("\ufeff")
        if bare.startswith("[") and bare.endswith("]"):
            section = bare.lower()
            continue
        if section != "[events]":
            continue
        m = re.match(r"^(Format|Dialogue|Comment)(\s*:\s*)", line, re.I)
        if not m:
            continue
        rest = line[m.end():]
        if m.group(1).lower() == "format":
            fields = [f.strip().lower() for f in rest.split(",")]
            continue
        if not fields or "start" not in fields or "end" not in fields or "text" not in fields:
            continue
        parts = rest.split(",", len(fields) - 1)
        if len(parts) < len(fields) or not ASS_TIME.match(parts[fields.index("start")]) \
                or not ASS_TIME.match(parts[fields.index("end")]):
            continue
        get = lambda name: parts[fields.index(name)] if name in fields else ""
        events.append({"row": n, "head": line[:m.end()], "parts": parts, "fields": fields,
                       "kind": m.group(1).lower(), "text": get("text"), "style": get("style"),
                       "effect": get("effect"), "s": ass_sec(get("start")), "e": ass_sec(get("end"))})
    if not events:
        return None
    for event in events:
        event["role"] = ass_kind(event)
    # One line for each moment's words, however many layers draw it; in time order, as SRT lines are.
    groups = {}
    for event in events:
        if event["role"] == "dialogue":
            key = (round(event["s"], 3), round(event["e"], 3), ass_plain(event["text"]))
            groups.setdefault(key, []).append(event)
    lines = [{"s": s, "e": e, "text": plain or "…", "members": members}
             for (s, e, plain), members in groups.items()]
    lines.sort(key=lambda line: (line["s"], line["members"][0]["row"]))
    return {"rows": rows, "events": events, "lines": lines, "bom": raw.startswith(b"\xef\xbb\xbf")}


def write_ass(ass, out, path):
    """Writes the file again with each event's new times, and returns how many were moved with the dialogue."""
    lines = ass["lines"]
    new = {}
    for line, o in zip(lines, out):
        for event in line["members"]:
            new[event["row"]] = (o["s"], o["e"])
    # What isn't spoken moves by the shift of the scene around it: the median shift of the lines spoken within
    # a minute of it (or of the nearest nine), placed by their words. Not by its next neighbours alone: in a
    # file out of sync, each line is off by its own error (up to 2 s either way), which a sign would take on.
    anchors = sorted((line["s"], o["s"] - line["s"]) for line, o in zip(lines, out) if not o.get("guess")) or \
        sorted((line["s"], o["s"] - line["s"]) for line, o in zip(lines, out))

    def shift_at(t):
        near = [sh for a, sh in anchors if abs(a - t) <= 60]
        if len(near) < 5:
            near = [sh for a, sh in sorted(anchors, key=lambda anchor: abs(anchor[0] - t))[:9]]
        near.sort()
        mid = len(near) // 2
        return near[mid] if len(near) % 2 else (near[mid - 1] + near[mid]) / 2
    moved = 0
    for event in ass["events"]:
        if event["role"] == "moved":
            sh = shift_at(event["s"])
            new[event["row"]] = (max(0.0, event["s"] + sh), max(0.0, event["e"] + sh))
            moved += 1
    rows = list(ass["rows"])
    for event in ass["events"]:
        if event["row"] not in new:
            continue
        s, e = new[event["row"]]
        parts = list(event["parts"])
        for name, value in (("start", s), ("end", e)):
            at = event["fields"].index(name)
            old = parts[at]
            parts[at] = old.replace(old.strip(), ass_stamp(value, old), 1)
        cr = "\r" if rows[event["row"]].endswith("\r") else ""
        rows[event["row"]] = event["head"] + ",".join(parts) + cr
    text = "\n".join(rows)
    if text.startswith("\ufeff"):
        text = text[1:]
    with open(path, "w", encoding="utf-8-sig" if ass["bom"] else "utf-8", newline="") as f:
        f.write(text)
    return moved


def start_audio(video, stream):
    """FFmpeg pulling the dialogue out as 16 kHz mono floats, started at once so it runs while models load."""
    probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", f"{stream}", "-show_entries",
                            "stream=channel_layout,channels", "-of", "csv=p=0", video],
                           capture_output=True, text=True, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    # ffprobe prints "6,5.1" (channels, then layout) or "2,stereo"; the layout may be missing.
    parts = [p.strip() for p in (probe.stdout.strip().splitlines() or [""])[0].split(",")]
    channels = next((int(p) for p in parts if p.isdigit()), 2)
    layout = next((p for p in parts if p and not p.isdigit()), "")
    centre = any(k in layout for k in ("5.1", "7.1", "6.1", "3.0", "4.0", "3.1", "5.0", "7.0"))
    pan = "pan=mono|c0=FC" if centre else "pan=mono|c0=0.5*c0+0.5*c1" if channels >= 2 else "anull"
    proc = subprocess.Popen(["ffmpeg", "-v", "error", "-nostdin", "-i", video, "-map", f"0:{stream}", "-vn",
                             "-af", pan, "-ac", "1", "-ar", str(SR), "-f", "f32le", "pipe:1"],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    box = {}

    def drain():
        box["raw"] = proc.stdout.read()
        box["err"] = proc.stderr.read().decode("utf-8", "replace")
        proc.wait()
    thread = threading.Thread(target=drain, daemon=True)
    thread.start()
    return proc, thread, box


STREAMS = 32        # stretches of the film the voice detector hears at once
WARM = 1875         # frames of 32 ms each stretch first hears of the one before it (60 s), to settle its memory


def speech_regions(audio, torch):
    """Where someone speaks: Silero VAD over 32 stretches of the film at once, each first hearing the 60 s
    before it so it starts as it would have if it had heard everything before (measured on Sicario: 1,081 of
    1,082 regions the same as one long pass, in 11 s instead of 48), then Silero's own rules, unchanged:
    speech from 0.35, silence below 0.20 for 250 ms, at least 250 ms long, 150 ms of margin."""
    from silero_vad import load_silero_vad
    model = load_silero_vad()
    W = 512
    n = len(audio) // W
    if n == 0:
        return []
    frames = torch.from_numpy(audio[: n * W].reshape(n, W))
    B = min(STREAMS, max(1, n // 2000))
    part = -(-n // B)
    warm = WARM
    idx = []
    for b in range(B):
        a = max(0, b * part - (warm if b else 0))
        idx.append((a, b * part, min(n, (b + 1) * part)))
    steps = max(z - a for a, _, z in idx)
    probs = torch.zeros(B, steps)
    model.reset_states()
    zero = torch.zeros(W)
    with torch.no_grad():
        for t in range(steps):
            x = torch.stack([frames[a + t] if a + t < z else zero for a, _, z in idx])
            probs[:, t] = model(x, SR).squeeze(1)
            if t % 2000 == 0:
                progress("speech", t / steps)
    p = torch.cat([probs[b, s - a: z - a] for b, (a, s, z) in enumerate(idx)]).tolist()

    # Silero's own rules turn the probabilities into speech regions, unchanged: it is handed them one by
    # one, as if it were working them out itself.
    from silero_vad import get_speech_timestamps

    class Known:
        def __init__(self, values):
            self.values, self.at = values, 0

        def reset_states(self):
            self.at = 0

        def __call__(self, chunk, sr):
            v = self.values[self.at] if self.at < len(self.values) else 0.0
            self.at += 1
            return torch.tensor([[v]])

    return get_speech_timestamps(torch.from_numpy(audio[: n * W]), Known(p), sampling_rate=SR, threshold=0.35,
                                 min_silence_duration_ms=250, speech_pad_ms=150, return_seconds=True)


def transcribe(audio, speech, models, language, torch):
    from qwen_asr import Qwen3ASRModel
    chunks, cur = [], None
    for s in speech:
        if cur and s["end"] - cur[0] <= 30.0:
            cur[1] = s["end"]
        else:
            if cur:
                chunks.append(cur)
            cur = [s["start"], s["end"]]
    if cur:
        chunks.append(cur)
    model = Qwen3ASRModel.from_pretrained(
        os.path.join(models, "asr"), dtype=torch.bfloat16, device_map="cuda:0",
        max_inference_batch_size=16, max_new_tokens=384,
        forced_aligner=os.path.join(models, "aligner"),
        forced_aligner_kwargs=dict(dtype=torch.bfloat16, device_map="cuda:0"),
    )
    words, texts = [], []
    B = 16  # as tested: a different batch pads the clips differently, and the transcript can change
    for i in range(0, len(chunks), B):
        batch = chunks[i:i + B]
        clips = [(audio[int(a * SR):int(b * SR)], SR) for a, b in batch]
        results = model.transcribe(audio=clips, language=[language] * len(clips), return_time_stamps=True)
        for (a, b), r in zip(batch, results):
            texts.append({"start": a, "end": b, "text": r.text})
            for it in (r.time_stamps or []):
                words.append({"w": it.text, "s": round(a + it.start_time, 3), "e": round(a + it.end_time, 3)})
        progress("words", min(1.0, (i + B) / max(1, len(chunks))))
    del model
    torch.cuda.empty_cache()
    return words, texts


def pair(lines, words, chunks, speech, models, torch, np):
    """Steps 4-7 above. Returns each line's new start and end, and what was found."""
    G = len(lines)
    norm = lambda x: re.sub(r"[^\w]", "", x.lower())
    # Punctuation from the recogniser's text, put back on the timed words.
    for ch in chunks:
        toks = ch["text"].split()
        ws = [w for w in words if ch["start"] - 0.01 <= w["s"] <= ch["end"] + 0.01]
        p = 0
        for w in ws:
            for q in range(p, min(p + 4, len(toks))):
                if norm(toks[q]) == norm(w["w"]):
                    w["d"] = toks[q]
                    p = q + 1
                    break
    for w in words:
        w.setdefault("d", w["w"])

    # Words the aligner stacked on one instant (or gave no length) are re-timed from the words around them:
    # forward from where it put them if someone speaks there, else just before the next word it placed.
    def reliable(k):
        w = words[k]
        if w["e"] - w["s"] < 0.02:
            return False
        if k > 0 and abs(words[k - 1]["s"] - w["s"]) < 0.005:
            return False
        if k + 1 < len(words) and abs(words[k + 1]["s"] - w["s"]) < 0.005:
            return False
        return True
    ok = [reliable(k) for k in range(len(words))]
    k = 0
    while k < len(words):
        if ok[k]:
            k += 1
            continue
        a = k
        while k < len(words) and not ok[k]:
            k += 1
        lo = words[a - 1]["e"] if a > 0 else max(0.0, words[a]["s"] - 0.3)
        hi = words[k]["s"] if k < len(words) else words[a]["s"] + 0.3 * (k - a)
        t0 = words[a]["s"]
        in_speech = any(r["start"] - 0.1 <= t0 <= r["end"] for r in speech)
        step = min(0.28, max(0.05, (hi - (t0 if in_speech else lo)) / (k - a)))
        for q in range(a, k):
            s_ = t0 + step * (q - a) if in_speech else max(lo, hi - step * (k - q))
            words[q]["s"], words[q]["e"] = s_, s_ + step * 0.85

    # Phrases.
    phrases, cur = [], []
    for k, w in enumerate(words):
        cur.append(w)
        nxt = words[k + 1] if k + 1 < len(words) else None
        gap = (nxt["s"] - w["e"]) if nxt else 99
        if gap >= 0.30 or re.search(r"[.?!…,;:]$", w["d"]) or w["e"] - cur[0]["s"] > 6:
            phrases.append({"s": cur[0]["s"], "e": cur[-1]["e"], "words": cur, "text": " ".join(x["d"] for x in cur)})
            cur = []
    J = len(phrases)
    if J == 0:
        raise RuntimeError("No speech was heard in this sound track.")

    # Rough whole-film fit.
    FR = 0.01
    n = int(max(phrases[-1]["e"], lines[-1]["e"] * 1.05) / FR) + 200
    sp = np.zeros(n)
    for w in words:
        sp[int(w["s"] / FR):int(w["e"] / FR) + 1] = 1
    N = 1 << int(np.ceil(np.log2(2 * n)))
    sp_f = np.fft.rfft(sp, N)

    def subs_frames(ratio, sel=None):
        a = np.zeros(n)
        for i, l in enumerate(lines):
            if sel is not None and not sel[i]:
                continue
            a[int(l["s"] * ratio / FR):max(int(l["s"] * ratio / FR) + 1, int(l["e"] * ratio / FR))] = 1
        return a

    def best_offset(sub, mask=None, max_off=120.0):
        b = np.where(sub > 0, 1.0, -1.0)
        if mask is not None:
            b = b * mask
        c = np.fft.irfft(sp_f * np.conj(np.fft.rfft(b, N)), N)
        lim = int(max_off / FR)
        cand = np.concatenate([c[N - lim:], c[:lim + 1]])
        return (int(np.argmax(cand)) - lim) * FR, float(cand.max())

    ratio, off, best = 1.0, 0.0, -1e18
    for r in (1.0, 25 / 23.976, 23.976 / 25, 24 / 23.976, 23.976 / 24, 25 / 24, 24 / 25):
        o, s = best_offset(subs_frames(r))
        if s > best:
            ratio, off, best = r, o, s
    pts = []
    for a in np.arange(0, lines[-1]["s"], 600):
        sel = [a <= l["s"] < a + 600 for l in lines]
        if sum(sel) < 20:
            continue
        mask = np.zeros(n)
        mask[int(a * ratio / FR):int((a + 600) * ratio / FR)] = 1
        o, _ = best_offset(subs_frames(ratio, sel), mask, 20)
        pts.append((a + 300, o - off))
    slope, inter = (np.polyfit([p[0] for p in pts], [p[1] for p in pts], 1) if len(pts) >= 3 else (0.0, 0.0))
    for l in lines:
        l["p"] = l["s"] * ratio + off + slope * l["s"] + inter
    progress("pairing", 0.1)

    # Meaning.
    WIN, MAXP, MAXSPAN = 7.0, 6, 12.0
    starts = np.array([p["s"] for p in phrases])

    def spans_near(t):
        j0 = int(np.searchsorted(starts, t - WIN - 0.5))
        j1 = int(np.searchsorted(starts, t + WIN + 0.5))
        for j in range(j0, min(j1, J)):
            for m in range(1, MAXP + 1):
                if j + m > J or phrases[j + m - 1]["e"] - phrases[j]["s"] > MAXSPAN:
                    break
                if m > 1 and phrases[j + m - 1]["s"] - phrases[j + m - 2]["e"] > 2.5:
                    break
                yield j, m

    span_keys, line_keys, cands = {}, {}, []
    for i in range(G):
        for k in (1, 2, 3):
            if i + k > G or (k > 1 and lines[i + k - 1]["s"] - lines[i + k - 2]["e"] > 1.0):
                break
            line_keys.setdefault((i, k), " ".join(lines[x]["text"] for x in range(i, i + k)))
            for j, m in spans_near(lines[i]["p"]):
                span_keys.setdefault((j, m), " ".join(phrases[x]["text"] for x in range(j, j + m)))
                cands.append((i, k, j, m))
    from sentence_transformers import SentenceTransformer
    model = SentenceTransformer(os.path.join(models, "meaning"), device="cuda")
    model.half()
    lk, sk = list(line_keys), list(span_keys)
    le = model.encode([line_keys[x] for x in lk], batch_size=256, normalize_embeddings=True, convert_to_numpy=True)
    progress("pairing", 0.3)
    se = model.encode([span_keys[x] for x in sk], batch_size=256, normalize_embeddings=True, convert_to_numpy=True)
    li = {x: q for q, x in enumerate(lk)}
    si = {x: q for q, x in enumerate(sk)}
    progress("pairing", 0.6)

    # Pairing.
    TAU, TIME_W, GROUP, SKIPW, EXTRA = 0.50, 0.015, 0.06, 0.015, 0.01
    NEG = -1e9
    score = np.full((G + 1, J + 1), NEG)
    back = np.zeros((G + 1, J + 1, 3), dtype=np.int32)
    score[0, :] = 0.0
    back[0, 1:] = (2, 0, 1)
    by_i = {}
    for c in cands:
        by_i.setdefault(c[0], []).append(c)
    words_in = [len(p["words"]) for p in phrases]
    for i in range(1, G + 1):
        row = score[i - 1].copy()
        brow = np.zeros((J + 1, 3), dtype=np.int32)
        brow[:] = (1, 1, 0)
        for k in (1, 2, 3):
            s0 = i - k
            if s0 < 0:
                continue
            for (_, kk, j, m) in by_i.get(s0, []):
                if kk != k:
                    continue
                sim = float(le[li[(s0, k)]] @ se[si[(j, m)]])
                if sim < TAU - 0.05:
                    continue
                if k > 1 and sum(words_in[j:j + m]) < 2 * k:
                    continue
                g = (sim - TAU) * (1 + 0.5 * (k - 1)) - TIME_W * abs(phrases[j]["s"] - lines[s0]["p"]) \
                    - GROUP * (k - 1) - EXTRA * (m - 1)
                v = score[s0, j] + g
                if v > row[j + m]:
                    row[j + m] = v
                    brow[j + m] = (3, k, m)
        for j in range(1, J + 1):
            v = row[j - 1] - SKIPW * words_in[j - 1]
            if v > row[j]:
                row[j] = v
                brow[j] = (2, 0, 1)
        score[i] = row
        back[i] = brow
    pairs = []
    i, j = G, J
    while i > 0:
        kind, k, m = back[i, j]
        if kind == 3:
            s0, j0 = i - k, j - m
            pairs.append((s0, k, j0, m, float(le[li[(s0, k)]] @ se[si[(j0, m)]])))
            i, j = s0, j0
        elif kind == 2:
            j -= 1
        else:
            i -= 1
    pairs.reverse()
    progress("pairing", 0.8)

    # Times.
    out = [None] * G
    for s0, k, j0, m, sim in pairs:
        ws = [w for p in phrases[j0:j0 + m] for w in p["words"]]
        if k == 1:
            out[s0] = {"s": ws[0]["s"], "e": ws[-1]["e"], "sim": sim}
            continue
        # Several lines share these words: cut them where each part best means what its line says.
        cut = [0]
        for x in range(s0, s0 + k - 1):
            lo_q = cut[-1] + 1
            hi_q = len(ws) - (k - (x - s0) - 1)
            qs = list(range(lo_q, hi_q + 1))
            if not qs:
                cut.append(min(len(ws) - 1, lo_q))
                continue
            head = [" ".join(w["d"] for w in ws[cut[-1]:q]) for q in qs]
            tail = [" ".join(w["d"] for w in ws[q:]) for q in qs]
            rest = " ".join(lines[y]["text"] for y in range(x + 1, s0 + k))
            eh = model.encode(head + tail, batch_size=128, normalize_embeddings=True, convert_to_numpy=True)
            e_line, e_rest = model.encode([lines[x]["text"], rest], normalize_embeddings=True, convert_to_numpy=True)
            q_n = len(qs)
            val = eh[:q_n] @ e_line + eh[q_n:] @ e_rest + np.array(
                [0.05 if ws[q]["s"] - ws[q - 1]["e"] >= 0.15 else 0.0 for q in qs])
            cut.append(qs[int(np.argmax(val))])
        cut.append(len(ws))
        for q, x in enumerate(range(s0, s0 + k)):
            part = ws[cut[q]:cut[q + 1]] or ws[-1:]
            out[x] = {"s": part[0]["s"], "e": part[-1]["e"], "sim": sim, "shared": True}

    # Leftovers: lines and speech both left unpaired between the same neighbours are each other.
    used = set()
    for s0, k, j0, m, sim in pairs:
        used.update(range(j0, j0 + m))
    x = 0
    while x < G:
        if out[x]:
            x += 1
            continue
        y = x
        while y < G and not out[y]:
            y += 1
        before = out[x - 1]["e"] if x > 0 and out[x - 1] else lines[x]["p"] - WIN
        after = out[y]["s"] if y < G and out[y] else lines[y - 1]["p"] + WIN
        free = [q for q in range(J) if q not in used and before - 0.05 <= phrases[q]["s"]
                and phrases[q]["e"] <= after + 0.05 and abs(phrases[q]["s"] - lines[x]["p"]) <= WIN + 3]
        run = list(range(x, y))
        if free and len(free) == len(run) and len(run) <= 3:
            for ln, q in zip(run, free):
                out[ln] = {"s": phrases[q]["words"][0]["s"], "e": phrases[q]["words"][-1]["e"], "sim": None,
                           "leftover": True}
                used.add(q)
        elif free and len(run) == 1 and phrases[free[-1]]["e"] - phrases[free[0]]["s"] <= 8:
            out[x] = {"s": phrases[free[0]]["words"][0]["s"], "e": phrases[free[-1]]["words"][-1]["e"],
                      "sim": None, "leftover": True}
            used.update(free)
        x = y

    # A line whose first word the recogniser missed starts where the voice detector heard speech begin.
    reg_starts = np.array([r["start"] for r in speech]) if speech else np.array([])
    starts_w = {w["s"]: q for q, w in enumerate(words)}

    def onset(first_s, prev_e):
        if not len(reg_starts):
            return first_s
        r = int(np.searchsorted(reg_starts, first_s + 1e-6)) - 1
        if r < 0 or speech[r]["end"] < first_s - 0.05:
            return first_s
        a = max(speech[r]["start"] + 0.15, prev_e + 0.1)
        return a if first_s - 1.5 <= a < first_s - 0.6 else first_s
    for x in range(G):
        o = out[x]
        if not o or o.get("shared"):
            continue
        q = starts_w.get(o["s"])
        if q is not None:
            o["s"] = onset(o["s"], words[q - 1]["e"] if q > 0 else 0.0)

    # Lines still unplaced: by their placed neighbours.
    for x in range(G):
        if out[x]:
            continue
        prev = next((y for y in range(x - 1, -1, -1) if out[y] and not out[y].get("guess")), None)
        nxt = next((y for y in range(x + 1, G) if out[y]), None)
        shift = lambda y: out[y]["s"] - lines[y]["s"]
        if prev is None and nxt is None:
            sh = lines[x]["p"] - lines[x]["s"]
        elif prev is None:
            sh = shift(nxt)
        elif nxt is None:
            sh = shift(prev)
        else:
            w_ = (lines[x]["s"] - lines[prev]["s"]) / max(1e-6, lines[nxt]["s"] - lines[prev]["s"])
            sh = shift(prev) * (1 - w_) + shift(nxt) * w_
        out[x] = {"s": lines[x]["s"] + sh, "e": lines[x]["e"] + sh, "sim": None, "guess": True}

    # Speech times to screen times.
    LEAD, LINGER, GAP, CPS, MINLEN = 0.04, 0.45, 0.085, 20.0, 1.0
    for o in out:
        if not o.get("guess"):
            o["s"] -= LEAD
            o["e"] += LINGER
    for x in range(1, G):
        if out[x]["s"] < out[x - 1]["s"] + 0.2:
            out[x]["s"] = out[x - 1]["s"] + 0.2
    need = [max(MINLEN, len(l["text"]) / CPS) for l in lines]
    for x in range(G):
        o = out[x]
        o["speech_e"] = o["e"] - (0 if o.get("guess") else LINGER)
        nxt_s = out[x + 1]["s"] if x + 1 < G else 1e9
        o["e"] = min(max(o["e"], o["s"] + need[x]), nxt_s - GAP)
    # Only a line too quick to read at all starts a little before its first word (at most 0.25 s).
    for x in range(1, G):
        o, p = out[x], out[x - 1]
        short = max(0.9, len(lines[x]["text"]) / 25.0) - (o["e"] - o["s"])
        if short <= 0.05:
            continue
        floor_ = max(p["s"] + need[x - 1], p["speech_e"] + 0.15)
        take = min(short, 0.25, max(0.0, (o["s"] - GAP) - max(floor_, p["s"])))
        if take > 0:
            o["s"] -= take
            p["e"] = min(p["e"], o["s"] - GAP)
    for x in range(G):
        o = out[x]
        if o["e"] - o["s"] < 0.6:
            o["e"] = o["s"] + 0.6
        if x + 1 < G and o["e"] > out[x + 1]["s"] - GAP:
            out[x + 1]["s"] = o["e"] + GAP
    progress("pairing", 1.0)
    return out


def run(args):
    t0 = time.time()
    import numpy as np
    import torch
    if not torch.cuda.is_available():
        raise RuntimeError("Subtitle sync needs an NVIDIA graphics card, and none could be used.")
    ass = read_ass(args.subs, args.charset)
    lines = ass["lines"] if ass else read_lines(args.subs, args.charset)
    if not lines:
        raise RuntimeError("No spoken lines were found in the subtitles." if ass else
                           "No subtitle lines were found in the file.")
    language = LANGUAGES.get((args.language or "").lower())
    if language and language not in TIMED:
        raise RuntimeError(f"Siphon can't yet time words spoken in {language}.")
    proc, drain, box = start_audio(args.video, args.stream)
    progress("audio", 0.0)
    drain.join()
    if proc.returncode != 0 or not box.get("raw"):
        raise RuntimeError("Couldn't read the film's sound." + (" " + box.get("err", "")[-300:] if box.get("err") else ""))
    audio = np.frombuffer(box["raw"], dtype=np.float32).copy()
    del box["raw"]
    progress("audio", 1.0)
    speech = speech_regions(audio, torch)
    progress("speech", 1.0)
    if not speech:
        raise RuntimeError("No speech was heard in this sound track.")
    words, chunks = transcribe(audio, speech, args.models, language, torch)
    del audio
    out = pair(lines, words, chunks, speech, args.models, torch, np)
    moved = 0
    if ass:
        moved = write_ass(ass, out, args.out)
    else:
        with open(args.out, "w", encoding="utf-8", newline="\n") as f:
            for x, l in enumerate(lines):
                f.write(f"{x + 1}\n{stamp(out[x]['s'])} --> {stamp(out[x]['e'])}\n" + "\n".join(l["body"]) + "\n\n")
    guesses = [round(out[x]["s"], 2) for x in range(len(lines)) if out[x].get("guess")]
    say(event="done", lines=len(lines), by_words=len(lines) - len(guesses), placed=len(guesses),
        placed_at=guesses[:50], moved=moved, seconds=round(time.time() - t0, 1))


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="mode", required=True)
    s = sub.add_parser("setup")
    s.add_argument("--models", required=True)
    r = sub.add_parser("run")
    r.add_argument("--models", required=True)
    r.add_argument("--video", required=True)
    r.add_argument("--stream", required=True)
    r.add_argument("--language")
    r.add_argument("--subs", required=True)
    r.add_argument("--charset")
    r.add_argument("--out", required=True)
    args = parser.parse_args()
    try:
        if args.mode == "setup":
            setup(args.models)
        else:
            run(args)
    except Exception as err:  # noqa: BLE001 - every failure is reported as one line
        say(event="error", message=str(err) or err.__class__.__name__)
        sys.exit(1)


if __name__ == "__main__":
    main()
