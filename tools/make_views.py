#!/usr/bin/env python3
"""Cut turned-head views for a persona from LivePortrait frames.

Each view is the portrait with the face swapped for a turned one (a few
degrees of LivePortrait rotate_yaw), plus its own mouth and eye patches.
The app crossfades between neighbouring views as the head turns, so a turn
needs no side photo and no morph.

    T=~/.local/share/rex-static/tools
    for y in 4 -4 8 -8; do
        $T/venv/bin/python -I tools/make_frames.py assets/miles.png $T/out/miles-views/y${y/-/n} --yaw $y
    done
    $T/venv/bin/python -I tools/make_views.py assets/miles.png MILES $T/out/miles-views assets/faces/miles/views

The face swap covers the persona's outline from src/face.rs (SPEC is its
name there) plus a margin, so the turned head keeps its own edge against
the white backdrop; the app moves the outline to that edge. The fade back
to the portrait sits out in the backdrop, where both are white. Writes, per view: face.png (the swapped
area, RGBA, cropped), the patches and faces.json.
"""
import argparse
import json
import os
import re
import sys

import cv2
import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import make_patches as mp  # noqa: E402

# How far past the outline (px) the swap reaches, and over how many px it
# fades back to the portrait.
GROW = 28
FADE = 10


def outline(spec):
    src = open(os.path.join(os.path.dirname(__file__), "..", "src", "face.rs")).read()
    blk = src.split(f"pub const {spec}: Spec")[1].split("outline: &[")[1].split("],")[0]
    pts = re.findall(r"\(([\d.]+), ([\d.]+)\)", blk)
    return np.array([[float(x), float(y)] for x, y in pts], np.int32)


def swap_mask(shape, poly):
    m = np.zeros(shape[:2], np.uint8)
    cv2.fillPoly(m, [poly], 255)
    m = cv2.erode(cv2.dilate(m, mp.disk(GROW)), mp.disk(FADE // 2))
    soft = cv2.GaussianBlur(m.astype(np.float32) / 255.0, (0, 0), FADE / 2.5)
    return np.where(soft > 0.02, np.maximum(soft, m / 255.0), 0.0)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("portrait")
    ap.add_argument("spec")
    ap.add_argument("frames", help="folder with one frame folder per view")
    ap.add_argument("out")
    a = ap.parse_args()

    base = mp.load(a.portrait)
    poly = outline(a.spec)
    for view in sorted(d for d in os.listdir(a.frames) if os.path.isdir(os.path.join(a.frames, d))):
        src = os.path.join(a.frames, view)
        out = os.path.join(a.out, view)
        turned = mp.load(os.path.join(src, "rest.png"))
        al = swap_mask(base.shape, poly)
        ys, xs = np.nonzero(al > 0)
        x0, y0, x1, y1 = xs.min(), ys.min(), xs.max() + 1, ys.max() + 1
        crop = al[y0:y1, x0:x1]
        face = mp.colour_fix(turned[y0:y1, x0:x1].astype(np.float32),
                             base[y0:y1, x0:x1].astype(np.float32), crop)
        view_img = base.astype(np.float32).copy()
        view_img[y0:y1, x0:x1] = view_img[y0:y1, x0:x1] * (1 - crop[..., None]) + face * crop[..., None]
        view_img = view_img.round().clip(0, 255).astype(np.int16)
        print(f"{view}: face {x1 - x0}x{y1 - y0} at {x0},{y0}")
        man = mp.cut(view_img, src, out, debug=False)
        rgba = np.dstack([face, crop * 255]).round().clip(0, 255).astype(np.uint8)
        Image.fromarray(rgba, "RGBA").save(os.path.join(out, "face.png"), optimize=True)
        man["patches"]["face"] = [int(x0), int(y0)]
        with open(os.path.join(out, "faces.json"), "w") as f:
            json.dump(man, f, indent=1)
        Image.fromarray(view_img.astype(np.uint8)).save(os.path.join(a.frames, f"{view}-view.png"))


if __name__ == "__main__":
    main()
