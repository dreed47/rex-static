#!/usr/bin/env python3
"""Make a persona's expression frames with LivePortrait, offline.

Runs the expression editor from ComfyUI-AdvancedLivePortrait on its own,
without ComfyUI, on the CPU if there's no CUDA card. Each frame is the same
portrait with only the expression changed, so the face keeps its identity.

Setup (once): see tools/README.md. Then:

    ~/.local/share/rex-static/tools/venv/bin/python tools/make_frames.py \
        assets/miles.png OUT_DIR [--only rest,aa] [--blink -12]

Safety: only .safetensors weights are loaded. The add-on's YOLO face finder
(a pickle file, which can run code when loaded) is replaced with OpenCV's
bundled Haar cascade, and the pickle-based modules it imports are stubbed out.
"""
import argparse
import json
import os
import sys
import tempfile
import time
import types

TOOLS = os.path.expanduser("~/.local/share/rex-static/tools")
ADDON = os.path.join(TOOLS, "alp")
MODELS = os.path.join(TOOLS, "models")

# Expression values, straight into run(). Started from issue #1's table;
# the mouth values are smaller after a live test.
FRAMES = {
    "rest": {},
    "blink06": {"blink": -6},
    "blink10": {"blink": -10},
    "blink14": {"blink": -14},
    # Mouth shapes, kept small: real speech rarely opens wide, and small
    # steps blend into each other without a double outline.
    "aa": {"aaa": 45},
    "ah": {"aaa": 30},
    "ee": {"aaa": 15, "eee": 3},
    "ih": {"aaa": 10},
    "oh": {"aaa": 35, "woo": 10},
    "oo": {"aaa": 12, "woo": 12},
    "mm": {"aaa": -10},
    "ff": {"aaa": 6, "eee": 3},
    "look_l14": {"pupil_x": -14},
    "look_r14": {"pupil_x": 14},
}

ARGS = ("rotate_pitch", "rotate_yaw", "rotate_roll", "blink", "eyebrow", "wink",
        "pupil_x", "pupil_y", "aaa", "eee", "woo", "smile")


def stub_host(tmp):
    """Stand-ins for the ComfyUI modules the add-on imports."""
    fp = types.ModuleType("folder_paths")
    fp.models_dir = MODELS
    fp.output_directory = tmp
    fp.get_folder_paths = lambda m: [os.path.join(MODELS, m)]
    fp.get_temp_directory = lambda: tmp
    fp.get_save_image_path = lambda *a, **k: (tmp, "frame", 0, "", "frame")
    sys.modules["folder_paths"] = fp

    def load_torch_file(path, *a, **k):
        if not path.endswith(".safetensors"):
            raise RuntimeError(f"refusing to load non-safetensors weights: {path}")
        import safetensors.torch
        return safetensors.torch.load_file(path, device="cpu")

    comfy = types.ModuleType("comfy")
    utils = types.ModuleType("comfy.utils")
    utils.load_torch_file = load_torch_file
    comfy.utils = utils
    sys.modules["comfy"] = comfy
    sys.modules["comfy.utils"] = utils

    def refuse(*a, **k):
        raise RuntimeError("pickle-based loading is disabled")

    # YOLO face finder and saved-expression files both load pickles.
    ultra = types.ModuleType("ultralytics")
    ultra.YOLO = refuse
    sys.modules["ultralytics"] = ultra
    dill = types.ModuleType("dill")
    dill.load = dill.dump = refuse
    sys.modules["dill"] = dill


def haar_bboxes(image_rgb):
    """Face box with OpenCV's bundled cascade; a centre box if none is found
    (stylised faces are sometimes missed)."""
    import cv2
    import numpy as np
    h, w = image_rgb.shape[:2]
    cascade = cv2.CascadeClassifier(cv2.data.haarcascades + "haarcascade_frontalface_default.xml")
    gray = cv2.cvtColor(image_rgb, cv2.COLOR_RGB2GRAY)
    found = cascade.detectMultiScale(gray, scaleFactor=1.1, minNeighbors=6, minSize=(120, 120))
    if len(found):
        x, y, bw, bh = max(found, key=lambda f: f[2] * f[3])
        print(f"face found at {x},{y} {bw}x{bh}", file=sys.stderr)
        return np.array([[x, y, x + bw, y + bh]], dtype=np.float32)
    print("no face found; using the centre box", file=sys.stderr)
    return np.array([[0.3 * w, 0.22 * h, 0.7 * w, 0.68 * h]], dtype=np.float32)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("portrait")
    ap.add_argument("out_dir")
    ap.add_argument("--only", help="comma-separated frame names")
    ap.add_argument("--blink", type=float, help="value for the fully shut blink14 frame")
    ap.add_argument("--set", help='extra frames as JSON, e.g. {"yaw_l8": {"rotate_yaw": -8}}')
    ap.add_argument("--yaw", type=float, default=0, help="turn every frame by this much (rotate_yaw)")
    ap.add_argument("--crop", type=float, default=1.7, help="crop factor around the face")
    a = ap.parse_args()

    frames = dict(FRAMES)
    if a.blink is not None:
        frames["blink14"] = {"blink": a.blink}
    if a.set:
        frames.update(json.loads(a.set))
    if a.only:
        frames = {k: frames[k] for k in a.only.split(",")}

    # torch first: it probes for a real dill, and the stub has no spec.
    import numpy as np
    import torch
    from PIL import Image
    tmp = tempfile.mkdtemp(prefix="rex-frames-")
    stub_host(tmp)
    sys.path.insert(0, TOOLS)
    alp = __import__("alp.nodes", fromlist=["nodes"])
    alp.LP_Engine.get_face_bboxes = lambda self, img: haar_bboxes(img)

    img = Image.open(a.portrait).convert("RGB")
    src = torch.from_numpy(np.asarray(img, dtype=np.float32) / 255.0).unsqueeze(0)
    os.makedirs(a.out_dir, exist_ok=True)
    ed = alp.ExpressionEditor()
    for name, vals in frames.items():
        t0 = time.time()
        args = {k: float(vals.get(k, 0)) for k in ARGS}
        args["rotate_yaw"] += a.yaw
        r = ed.run(**args, src_ratio=1.0, sample_ratio=1.0, sample_parts="OnlyExpression",
                   crop_factor=a.crop, src_image=src)
        out = r["result"][0][0] if isinstance(r, dict) else r[0][0]
        arr = np.clip(out.cpu().numpy() * 255.0, 0, 255).astype(np.uint8)
        Image.fromarray(arr).save(os.path.join(a.out_dir, f"{name}.png"))
        print(f"{name}: {time.time() - t0:.1f} s", file=sys.stderr)


if __name__ == "__main__":
    main()
