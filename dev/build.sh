#!/usr/bin/env bash
# Build Haiku.exe from this checkout into its own target dir and stage a run
# folder. Paths: dev/env.sh (HAIKU_WORK, HAIKU_MAIN, ...).
# Usage (from a checkout or worktree root):  dev/build.sh <name>
#   -> runs/<name>/Haiku.exe, with runs/<name>/game linked to $HAIKU_MAIN/game
#      (when there is one) and assets/ to this checkout's.
# It links $HAIKU_WORK/recomp-<name>/code (dev/recomp.sh <name>), else
# $HAIKU_MAIN/source/code.
set -euo pipefail
NAME=${1:?usage: dev/build.sh <name>}
HERE=$(cd "$(dirname "$0")/.." && pwd)
. "$HERE/dev/env.sh"
export CARGO_TARGET_DIR=$(wpath "$WORK/target-$NAME")
if [ -d "$WORK/recomp-$NAME/code" ]; then export FATES_RECOMP_DIR=$(wpath "$WORK/recomp-$NAME/code"); else export FATES_RECOMP_DIR=$(wpath "$MAIN/source/code"); fi
# native-final: PROFILE=ship builds the shipped profile (thin LTO)
PROFILE=${PROFILE:-release}
cargo build --manifest-path "$(wpath "$HERE/source/Cargo.toml")" --profile "$PROFILE" ${CARGO_EXTRA:-}
RUN="$HERE/runs/$NAME"
mkdir -p "$RUN"
cp "$WORK/target-$NAME/$PROFILE/Haiku.exe" "$RUN/Haiku.exe"
# the program had another name before it was named Haiku
rm -f "$RUN/fates-pc.exe"
# SDL3.dll etc. next to the exe if the tree has any runtime DLLs
for dll in "$HERE"/dist-dll/*.dll; do [ -e "$dll" ] && cp "$dll" "$RUN/"; done
if [ ! -e "$RUN/game" ] && [ -d "$MAIN/game" ]; then
  cmd //c mklink //J "$(wpath "$RUN/game")" "$(wpath "$MAIN/game")" >/dev/null
fi
"$HERE/dev/link-assets.sh" "$RUN"
echo "built $RUN/Haiku.exe"
