#!/usr/bin/env python3
"""Kokoro with phoneme timings, for lip sync.

Same output as piper_timed.py, so the app treats both alike. Reads text on
stdin. Writes raw 16-bit mono audio at 22050 Hz to stdout (Kokoro makes
24 kHz; it is resampled to match Piper), and the phoneme timings as a last
stderr line starting `ALIGN:`:

    [["h", 0, 1890], ["ə", 1890, 3012], ...]   # phoneme, first sample, end sample

Run with the Python of the kokoro venv that setup.sh makes. The model must
be an export with a `duration` output (kokoro-onnx release
model-files-v1.1); without one it still speaks, with an empty timing list.
"""
import argparse
import json
import sys

import numpy as np
from kokoro_onnx import Kokoro

OUT_RATE = 22050


def resample(x, src, dst):
    """Band-limited resample of a whole clip through the FFT."""
    if src == dst or len(x) == 0:
        return x
    n = int(round(len(x) * dst / src))
    spec = np.fft.rfft(x)
    keep = n // 2 + 1
    if keep <= len(spec):
        spec = spec[:keep]
    else:
        spec = np.concatenate([spec, np.zeros(keep - len(spec), dtype=spec.dtype)])
    return np.fft.irfft(spec, n) * (n / len(x))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("-m", "--model", required=True)
    ap.add_argument("--voices", required=True)
    ap.add_argument("--voice", required=True)
    ap.add_argument("--lang", default="en-us")
    ap.add_argument("--speed", type=float, default=1.0)
    a = ap.parse_args()

    text = sys.stdin.read().strip()
    k = Kokoro(a.model, a.voices)
    audio, rate, spoken = k.create_timed(
        text, voice=a.voice, speed=a.speed, lang=a.lang, sentence_pause=0.2, clause_pause=0.08
    )
    audio = resample(np.asarray(audio, dtype=np.float64), rate, OUT_RATE)
    pcm = (np.clip(audio, -1.0, 1.0) * 32767).astype("<i2")
    sys.stdout.buffer.write(pcm.tobytes())
    sys.stdout.buffer.flush()

    timings = [[t.phoneme, int(t.start * OUT_RATE), int(t.end * OUT_RATE)] for t in spoken]
    sys.stderr.write("\nALIGN:" + json.dumps(timings, ensure_ascii=False) + "\n")
    sys.stderr.flush()


if __name__ == "__main__":
    main()
