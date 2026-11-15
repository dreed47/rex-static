#!/usr/bin/env python3
"""Preview the patch-based face as an MP4: lip sync from Piper's phoneme
timings, plus blinks and glances. For judging the look before it goes into
the app.

    ~/.local/share/rex-static/tools/venv/bin/python tools/preview.py \
        assets/miles.png PATCH_DIR TIMINGS.json AUDIO.raw OUT.mp4 \
        [--pitch 0.84] [--cartoon]

AUDIO.raw is 22.05 kHz 16-bit mono from tools/piper_timed.py.
"""
import argparse
import json
import random
import subprocess
import tempfile
import wave

import numpy as np
from PIL import Image

RATE = 22050
FPS = 60

# espeak phoneme -> mouth shape. Stress and length marks follow their
# neighbour; punctuation closes the mouth.
SHAPES = {
    "aa": "aɑæɐ",
    "ah": "ɛɜɝʌ",
    "ee": "ij",
    "ih": "ɪəɚᵻtdnlszkɡghθðŋɾx",
    "oh": "oɔɒ",
    "oo": "uʊwɹrʃʒ",
    "mm": "mbp",
    "ff": "fv",
}
SHAPE_OF = {c: s for s, cs in SHAPES.items() for c in cs}
FOLLOW_NEXT = "ˈˌ"
FOLLOW_PREV = "ːˑ "
MOUTH = list(SHAPES)
EYES = ["blink06", "blink10", "blink14", "look_l14", "look_r14"]


def shape_segments(timings, pitch):
    """(start s, end s, shape) per phoneme, in output time."""
    segs = []
    for i, (ph, a, b) in enumerate(timings):
        if ph in FOLLOW_NEXT:
            nxt = next((p for p, _, _ in timings[i + 1:] if p not in FOLLOW_NEXT), "")
            shape = SHAPE_OF.get(nxt, "rest")
        elif ph in FOLLOW_PREV:
            shape = segs[-1][2] if segs else "rest"
        else:
            shape = SHAPE_OF.get(ph, "rest")
        segs.append((a / RATE / pitch, b / RATE / pitch, shape))
    return segs


def mouth_weights(segs, n, lead, sigma):
    """Mouth shape weights per video frame. Targets are laid out at 1 ms,
    moved earlier by `lead` (the mouth shapes a sound before you hear it),
    then blurred so each shape flows into the next instead of cutting."""
    ms = int(n / FPS * 1000) + 1
    tgt = {s: np.zeros(ms) for s in MOUTH}
    for a, b, shape in segs:
        if shape != "rest":
            i0, i1 = max(0, int((a - lead) * 1000)), max(0, int((b - lead) * 1000))
            tgt[shape][i0:i1] = 1.0
    k = np.arange(-3 * sigma, 3 * sigma + 1)
    g = np.exp(-0.5 * (k / sigma) ** 2)
    g /= g.sum()
    idx = (np.arange(n) * 1000 / FPS).astype(int).clip(0, ms - 1)
    out = [dict() for _ in range(n)]
    for s, t in tgt.items():
        v = np.convolve(t, g, mode="same")[idx]
        for f in np.nonzero(v > 0.01)[0]:
            out[f][s] = float(v[f])
    return out


def eye_track(n, seed):
    """Weights per frame for each eye patch: blinks and the odd glance."""
    rng = random.Random(seed)
    w = [dict() for _ in range(n)]
    t = rng.uniform(1.0, 3.0)
    while t < n / FPS:
        # Close and open over about 170 ms.
        stages = ["blink06", "blink10", "blink14", "blink14", "blink10", "blink06"]
        for f in range(int(t * FPS), min(n, int((t + 0.17) * FPS))):
            w[f] = {stages[min(5, int((f / FPS - t) / 0.17 * 6))]: 1.0}
        t += rng.uniform(2.5, 5.5)
    t = rng.uniform(3.0, 6.0)
    while t < n / FPS:
        side = rng.choice(["look_l14", "look_r14"])
        for f in range(int(t * FPS), min(n, int((t + 0.9) * FPS))):
            if not w[f]:
                u = f / FPS - t
                w[f] = {side: min(1.0, u / 0.12, (0.9 - u) / 0.12)}
        t += rng.uniform(6.0, 10.0)
    return w


def hold(weights, fps):
    """Cartoon timing: sample the weights every 1/fps s and hold them."""
    step = FPS / fps
    return [weights[int(int(f / step) * step)] for f in range(len(weights))]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("portrait")
    ap.add_argument("patches")
    ap.add_argument("timings")
    ap.add_argument("audio")
    ap.add_argument("out")
    ap.add_argument("--pitch", type=float, default=1.0)
    ap.add_argument("--cartoon", action="store_true", help="hold shapes at 12 fps (retro)")
    ap.add_argument("--lead", type=float, default=40, help="ms the mouth moves ahead of the sound")
    ap.add_argument("--blur", type=float, default=35, help="ms of smoothing between shapes")
    ap.add_argument("--seed", type=int, default=3)
    a = ap.parse_args()

    base = np.asarray(Image.open(a.portrait).convert("RGB"), np.float32)
    h, w = base.shape[:2]
    h2, w2 = h - h % 2, w - w % 2
    man = json.load(open(f"{a.patches}/faces.json"))["patches"]

    def patch(name):
        p = np.asarray(Image.open(f"{a.patches}/{name}.png"), np.float32)
        x, y = man[name]
        return p[..., :3], p[..., 3:] / 255.0, (x, y)

    mouth = {s: patch(f"mouth_{s}") for s in ["rest"] + MOUTH}
    eyes = {s: patch(f"eyes_{s}") for s in ["rest"] + EYES}

    raw = np.frombuffer(open(a.audio, "rb").read(), np.int16).astype(np.float32)
    if a.pitch != 1.0:
        n_out = int(len(raw) / a.pitch)
        raw = np.interp(np.arange(n_out) * a.pitch, np.arange(len(raw)), raw)
    n = int(len(raw) / RATE * FPS) + FPS // 2
    timings = json.load(open(a.timings))
    segs = shape_segments(timings, a.pitch)
    mw = mouth_weights(segs, n, a.lead / 1000, a.blur)
    if a.cartoon:
        mw = hold(mw, 12)
    ew = eye_track(n, a.seed)
    if a.cartoon:
        ew = hold(ew, 12)

    tmp = tempfile.mkdtemp(prefix="rex-preview-")
    with wave.open(f"{tmp}/a.wav", "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(RATE)
        pad = np.zeros(int(n * RATE / FPS) - len(raw) + RATE)
        wf.writeframes(np.clip(np.concatenate([raw, pad]), -32768, 32767).astype(np.int16).tobytes())
    ff = subprocess.Popen(
        ["/usr/bin/ffmpeg", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
         "-s", f"{w2}x{h2}", "-r", str(FPS), "-i", "-", "-i", f"{tmp}/a.wav",
         "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18", "-c:a", "aac", "-shortest", a.out],
        stdin=subprocess.PIPE)
    for f in range(n):
        img = base.copy()
        for group, weights in ((mouth, mw[f]), (eyes, ew[f])):
            rgb, al, (x, y) = group["rest"]
            mix = rgb * (1 - sum(weights.values()))
            for s, v in weights.items():
                mix = mix + group[s][0] * v
            ph, pw = rgb.shape[:2]
            img[y:y + ph, x:x + pw] = img[y:y + ph, x:x + pw] * (1 - al) + mix * al
        ff.stdin.write(img[:h2, :w2].clip(0, 255).astype(np.uint8).tobytes())
    ff.stdin.close()
    ff.wait()
    print(f"{a.out}: {n / FPS:.1f} s")


if __name__ == "__main__":
    main()
