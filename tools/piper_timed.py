#!/usr/bin/env python3
"""Piper with phoneme timings, for lip sync.

Reads text on stdin. Writes raw 16-bit mono audio to stdout, like
`piper --output-raw`, and the phoneme timings as JSON to the --align file
(or, without --align, as a last stderr line starting `ALIGN:`):

    [["h", 0, 1890], ["ə", 1890, 3012], ...]   # phoneme, first sample, end sample

Sample numbers count the audio on stdout, sentence gaps included.

Run with Piper's own Python, which needs the onnx package for timings
(`uv pip install --python ~/.local/share/uv/tools/piper-tts/bin/python onnx`).
Without onnx it still speaks and writes an empty timing list.
"""
import argparse
import json
import sys

from piper import PiperVoice, SynthesisConfig


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("-m", "--model", required=True)
    ap.add_argument("--align", help="file for the phoneme timings (default: stderr)")
    ap.add_argument("--length-scale", type=float, default=1.0)
    ap.add_argument("--sentence-silence", type=float, default=0.05)
    a = ap.parse_args()

    voice = PiperVoice.load(a.model, include_alignments=True)
    cfg = SynthesisConfig(length_scale=a.length_scale)
    gap = int(voice.config.sample_rate * a.sentence_silence)
    text = sys.stdin.read().strip()
    out = sys.stdout.buffer
    pos = 0
    timings = []
    for chunk in voice.synthesize(text, cfg, include_alignments=True):
        audio = chunk.audio_int16_bytes
        at = pos
        for al in chunk.phoneme_alignments or []:
            n = int(al.num_samples)
            if al.phoneme not in ("^", "$", "_"):
                timings.append([al.phoneme, at, at + n])
            at += n
        out.write(audio)
        pos += len(audio) // 2
        out.write(b"\0\0" * gap)
        pos += gap
    out.flush()
    if a.align:
        with open(a.align, "w") as f:
            json.dump(timings, f, ensure_ascii=False)
    else:
        sys.stderr.write("\nALIGN:" + json.dumps(timings, ensure_ascii=False) + "\n")
        sys.stderr.flush()


if __name__ == "__main__":
    main()
