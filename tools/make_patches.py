#!/usr/bin/env python3
"""Cut mouth and eye patches from a persona's LivePortrait frames.

Only the mouth and the eyes change between frames, so the app draws the
original portrait and pastes one small patch over each. The patches share
one mask per group, so the nose and cheeks never move or change colour.

    ~/.local/share/rex-static/tools/venv/bin/python tools/make_patches.py \
        assets/miles.png ~/.local/share/rex-static/tools/out/miles assets/faces/miles

Writes <group>_<frame>.png (RGBA, cropped) and faces.json with each patch's
position on the portrait. Also writes debug_mask.png to check the masks.
"""
import argparse
import json
import os

import cv2
import numpy as np
from PIL import Image

EYES = ("blink06", "blink10", "blink14", "look_l14", "look_r14")
MOUTH = ("aa", "ah", "ee", "ih", "oh", "oo", "mm", "ff")


def load(path):
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.int16)


def diff(a, b):
    return np.abs(a - b).max(axis=2)


def disk(r):
    return cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * r + 1, 2 * r + 1))


def face_box(rgb):
    cascade = cv2.CascadeClassifier(cv2.data.haarcascades + "haarcascade_frontalface_default.xml")
    gray = cv2.cvtColor(rgb.astype(np.uint8), cv2.COLOR_RGB2GRAY)
    found = cascade.detectMultiScale(gray, scaleFactor=1.1, minNeighbors=6, minSize=(120, 120))
    if len(found):
        return max(found, key=lambda f: f[2] * f[3])
    h, w = rgb.shape[:2]
    return (int(0.3 * w), int(0.22 * h), int(0.4 * w), int(0.4 * w))


def nostril_row(rgb, box):
    """Darkest row in a strip down the middle of the face, between the eyes
    and the mouth: the nostrils."""
    x, y, s, _ = box
    gray = cv2.cvtColor(rgb.astype(np.uint8), cv2.COLOR_RGB2GRAY).astype(np.float32)
    strip = gray[y + int(0.52 * s): y + int(0.75 * s), x + int(0.4 * s): x + int(0.6 * s)]
    rows = cv2.GaussianBlur(strip, (1, 5), 0).mean(axis=1)
    return y + int(0.52 * s) + int(np.argmin(rows))


def blobs(mask, keep):
    n, lab, stats, _ = cv2.connectedComponentsWithStats(mask, 8)
    order = sorted(range(1, n), key=lambda i: -stats[i, cv2.CC_STAT_AREA])[:keep]
    out = np.zeros_like(mask)
    for i in order:
        if stats[i, cv2.CC_STAT_AREA] > 30:
            out[lab == i] = 255
    return out


def hull(mask):
    out = np.zeros_like(mask)
    cnts, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    for c in cnts:
        cv2.fillConvexPoly(out, cv2.convexHull(c), 255)
    return out


def group_mask(rest, frames, thresh, region, close, keep, grow, feather):
    m = np.zeros(rest.shape[:2], np.uint8)
    for f in frames:
        m |= ((diff(f, rest) > thresh) * 255).astype(np.uint8)
    m &= region
    if close:
        m = cv2.morphologyEx(m, cv2.MORPH_CLOSE, disk(close))
    m = hull(blobs(m, keep))
    m = cv2.dilate(m, disk(grow))
    soft = cv2.GaussianBlur(m.astype(np.float32) / 255.0, (0, 0), feather)
    return np.clip(soft * 1.4, 0, 1) * (cv2.dilate(m, disk(feather * 2)) > 0)


def colour_fix(patch, base, alpha):
    """Shift the patch's colour to match the portrait around its edge, where
    LivePortrait's re-render drifts a little."""
    ring = (alpha > 0.05) & (alpha < 0.6)
    if ring.sum() < 20:
        return patch
    off = (base[ring] - patch[ring]).mean(axis=0)
    return np.clip(patch + off, 0, 255)


def cut(base, frames, out, debug=True):
    """Cut the patches for `base` (the image the app draws them on) from
    the frames in `frames`, into `out`."""
    rest = load(os.path.join(frames, "rest.png"))
    assert base.shape == rest.shape, "frames must match the portrait size"
    h, w = base.shape[:2]
    box = face_box(base)
    nose = nostril_row(base, box)
    below = np.zeros((h, w), np.uint8)
    below[nose + 2:] = 255
    above = 255 - below

    groups = {
        "mouth": (MOUTH, group_mask(rest, [load(f"{frames}/{n}.png") for n in MOUTH],
                                    40, below, 9, 1, 4, 3)),
        "eyes": (EYES, group_mask(rest, [load(f"{frames}/{n}.png") for n in EYES],
                                  30, above, 0, 2, 3, 2)),
    }
    os.makedirs(out, exist_ok=True)
    manifest = {"size": [w, h], "patches": {}}
    dbg = base.astype(np.float32).copy()
    for g, (names, alpha) in groups.items():
        ys, xs = np.nonzero(alpha > 0)
        x0, y0, x1, y1 = xs.min(), ys.min(), xs.max() + 1, ys.max() + 1
        al = alpha[y0:y1, x0:x1]
        tint = np.array([255, 0, 0] if g == "mouth" else [0, 160, 255], np.float32)
        dbg = dbg * (1 - 0.45 * alpha[..., None]) + tint * 0.45 * alpha[..., None]
        for n in ("rest",) + names:
            f = load(f"{frames}/{n}.png").astype(np.float32)[y0:y1, x0:x1]
            f = colour_fix(f, base[y0:y1, x0:x1].astype(np.float32), al)
            rgba = np.dstack([f, al * 255]).round().astype(np.uint8)
            Image.fromarray(rgba, "RGBA").save(os.path.join(out, f"{g}_{n}.png"), optimize=True)
            manifest["patches"][f"{g}_{n}"] = [int(x0), int(y0)]
        print(f"{g}: {x1 - x0}x{y1 - y0} at {x0},{y0}")
    if debug:
        cv2.line(dbg, (0, nose), (w, nose), (0, 255, 0), 1)
        Image.fromarray(dbg.clip(0, 255).astype(np.uint8)).save(os.path.join(out, "debug_mask.png"))
    return manifest


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("portrait")
    ap.add_argument("frames")
    ap.add_argument("out")
    a = ap.parse_args()
    manifest = cut(load(a.portrait), a.frames, a.out)
    with open(os.path.join(a.out, "faces.json"), "w") as f:
        json.dump(manifest, f, indent=1)


if __name__ == "__main__":
    main()
