#!/usr/bin/env bash
# Update Rex Static to the latest GitHub release: download its source,
# then run that release's setup.sh, which builds and installs it.
# Usage: ./update.sh [--force]   (--force rebuilds even if already current)
set -euo pipefail

repo="dreed47/rex-static"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/rex-static"
force=${1:-}

for p in /usr/bin/curl /usr/bin/tar; do
  [[ -x $p ]] || { echo "Missing: $p" >&2; exit 1; }
done

echo "Checking the latest release..."
tag=$(/usr/bin/curl -fsSL --proto '=https' --retry 3 \
  -H "Accept: application/vnd.github+json" \
  "https://api.github.com/repos/$repo/releases/latest" |
  sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
if [[ ! $tag =~ ^v[0-9]+(\.[0-9]+)*$ ]]; then
  echo "Couldn't read the latest release tag (got '${tag}')." >&2
  exit 1
fi

mkdir -p "$cache"
installed=$(cat "$cache/installed" 2>/dev/null || true)
if [[ $installed == "$tag" && $force != --force ]]; then
  echo "Already on $tag. Run ./update.sh --force to rebuild it."
  exit 0
fi

echo "Downloading $tag..."
src="$cache/src-$tag"
rm -rf "$src" "$src.tar.gz"
/usr/bin/curl -fL --proto '=https' --retry 3 -o "$src.tar.gz" \
  "https://github.com/$repo/archive/refs/tags/$tag.tar.gz"
mkdir "$src"
/usr/bin/tar -xzf "$src.tar.gz" -C "$src" --strip-components=1
rm "$src.tar.gz"

# Share one build folder between releases, so updates build only what changed.
mkdir -p "$cache/target"
ln -s "$cache/target" "$src/target"

(cd "$src" && ./setup.sh)

echo "$tag" > "$cache/installed"
# Keep only this release's source.
find "$cache" -maxdepth 1 -name 'src-v*' ! -name "src-$tag" -exec rm -rf {} +
echo "Updated to $tag."
