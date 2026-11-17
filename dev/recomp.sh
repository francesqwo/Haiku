#!/usr/bin/env bash
# Regenerate the recompiled game code with this checkout's overrides/ into
# $HAIKU_WORK/recomp-<name>/code (incremental). dev/build.sh <name> then
# links it. Paths: dev/env.sh (HAIKU_WORK, HAIKU_DUMP, ...).
# Usage (from a checkout or worktree root): dev/recomp.sh <name>
# overrides/amiibo_icons.c is made from the dump: run Haiku Setup's
# --apply-derived into this checkout first (README, Building from source).
set -euo pipefail
NAME=${1:?usage: dev/recomp.sh <name>}
HERE=$(cd "$(dirname "$0")/.." && pwd)
. "$HERE/dev/env.sh"
[ -f "$DUMP" ] || { echo "no dump at $DUMP (set HAIKU_DUMP to your decrypted Fates-USA.cxi)"; exit 1; }
[ -f "$HERE/overrides/amiibo_icons.c" ] || { echo "overrides/amiibo_icons.c is missing: make the derived files first (Haiku Setup --apply-derived)"; exit 1; }
R="$WORK/recomp-$NAME"
if [ ! -d "$R/code" ]; then
  mkdir -p "$R/code"
  # a checkout that already has recompiled code seeds the first build
  if [ -d "$MAIN/source/code" ] && [ "$MAIN/source/code" != "$R/code" ]; then
    echo "seeding $R/code from the shared build (one-time copy)"
    (cd "$MAIN/source/code" && find . -maxdepth 1 -type f -print0 | xargs -0 -r cp -t "$R/code/")
  fi
fi
[ -e "$R/Cargo.toml" ] || echo "# placeholder so fates_port does not write a project here" > "$R/Cargo.toml"
export CARGO_TARGET_DIR=$(wpath "$WORK/target-recomp-$NAME")
cargo build --manifest-path "$(wpath "$HERE/vendor/3dsrecomp/Cargo.toml")" --release --example fates_port
"$WORK/target-recomp-$NAME/release/examples/fates_port.exe" "$(wpath "$DUMP")" "$(wpath "$R")" "$(wpath "$HERE/vendor/zakuro")" "$(wpath "$HERE/overrides")"
echo "recompiled into $R/code"
