#!/usr/bin/env bash
# Set up Rex Static: check tools, install Piper, download voice and speech
# models (checked against known hashes), write an example config, build.
# Safe to run again: existing models and config are kept.
set -euo pipefail
cd "$(dirname "$0")"

data="${XDG_DATA_HOME:-$HOME/.local/share}/rex-static"
conf="${XDG_CONFIG_HOME:-$HOME/.config}/rex-static/config"
models="$data/models"
piper_base="https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US"
whisper_base="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

missing=()
for p in cargo uv curl sha256sum; do command -v "$p" >/dev/null || missing+=("$p"); done
for p in /usr/bin/pw-cat /usr/bin/curl /usr/bin/secret-tool; do [[ -x $p ]] || missing+=("$p"); done
command -v cmake >/dev/null || missing+=("cmake (whisper build)")
command -v clang >/dev/null || missing+=("clang (whisper build)")
if ((${#missing[@]})); then
  echo "Missing: ${missing[*]}" >&2
  echo "On Arch: sudo pacman -S --needed rust uv curl pipewire libsecret cmake clang" >&2
  exit 1
fi

if [[ ! -x $HOME/.local/bin/piper ]]; then
  echo "Installing Piper (uv tool install piper-tts)..."
  uv tool install piper-tts
fi

mkdir -p "$models"
fetch() { # url name sha256
  local dest="$models/$2"
  if [[ -f $dest ]]; then
    echo "$3  $dest" | sha256sum -c --quiet - || { echo "$dest exists but its hash is wrong; delete it and run again" >&2; exit 1; }
    return
  fi
  echo "Downloading $2..."
  curl -fL --proto '=https' --retry 3 -o "$dest.part" "$1"
  if ! echo "$3  $dest.part" | sha256sum -c --quiet -; then
    rm -f "$dest.part"; echo "Hash mismatch for $2" >&2; exit 1
  fi
  mv "$dest.part" "$dest"
}
fetch "$piper_base/ryan/high/en_US-ryan-high.onnx" en_US-ryan-high.onnx b3990d7606e183ec8dbfba70a4607074f162de1a0c412e0180d1ff60bb154eca
fetch "$piper_base/ryan/high/en_US-ryan-high.onnx.json" en_US-ryan-high.onnx.json c6d3b98f08315cb4bebf0d49d50fc4ff491b503c64b940cd3d5ca28543b48011
fetch "$piper_base/lessac/high/en_US-lessac-high.onnx" en_US-lessac-high.onnx 4cabf7c3a638017137f34a1516522032d4fe3f38228a843cc9b764ddcbcd9e09
fetch "$piper_base/lessac/high/en_US-lessac-high.onnx.json" en_US-lessac-high.onnx.json db42b97d9859f257bc1561b8ed980e7fb2398402050a74ddd6cbec931a92412f
fetch "$whisper_base/ggml-base.en.bin" ggml-base.en.bin a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002

if [[ ! -e $conf ]]; then
  mkdir -p "$(dirname "$conf")"
  (umask 077; cp config.example "$conf")
  echo "Wrote $conf (edit it to pick a persona and turn on tools)."
fi

cargo build --release

# Install the program and an app launcher entry.
bin="$HOME/.local/bin/rex-static"
icon="$HOME/.local/share/icons/rex-static.png"
desktop="$HOME/.local/share/applications/rex-static.desktop"
mkdir -p "$(dirname "$bin")" "$(dirname "$icon")" "$(dirname "$desktop")"
install -m 755 target/release/rex-static "$bin"
install -m 644 assets/rex.png "$icon"
cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Rex Static
Comment=Glitchy talking-head assistant
Exec=$bin
Icon=$icon
Terminal=false
Categories=Utility;
StartupWMClass=rex-static
DESKTOP
echo
echo "Done. Open Rex Static from your app launcher, or run $bin"
if [[ -z ${ANTHROPIC_API_KEY:-} ]] && ! /usr/bin/secret-tool lookup service rex-static key anthropic >/dev/null 2>&1; then
  echo "No Anthropic key yet. Store one with:"
  echo "  secret-tool store --label='Rex Static' service rex-static key anthropic"
fi
