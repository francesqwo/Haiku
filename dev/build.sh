#!/usr/bin/env bash
# Build Haiku.exe from this checkout into its own target dir and stage a run
# folder. Paths: dev/env.sh (HAIKU_WORK, HAIKU_MAIN, ...).
# Usage (from a checkout or worktree root):  dev/build.sh <name>
#   -> runs/<name>/Haiku.exe, with runs/<name>/game linked to $HAIKU_MAIN/game
#      (when there is one) and assets/ to this checkout's.
# oneexe: Haiku.exe holds the engine and Haiku's overrides; the game's code
# is the module dev/recomp.sh <name> builds ($HAIKU_WORK/recomp-<name>/module),
# copied here into runs/<name>/data/code, where Haiku.exe looks for it. A run
# folder whose data/ is a link to the main checkout's gets none (that would
# write into the main checkout): give such a run HAIKU_GAME_MODULE=<game.dll>.
set -euo pipefail
NAME=${1:?usage: dev/build.sh <name>}
HERE=$(cd "$(dirname "$0")/.." && pwd)
. "$HERE/dev/env.sh"
export CARGO_TARGET_DIR=$(wpath "$WORK/target-$NAME")
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
MODULE="$WORK/recomp-$NAME/module"
if [ -f "$MODULE/game.dll" ]; then
  if [ -L "$RUN/data" ] || { [ -e "$RUN/data" ] && [ "$(cd "$RUN/data" && pwd -P)" != "$(cd "$RUN" && pwd -P)/data" ]; }; then
    echo "runs/$NAME/data is a link: the module is not copied there (HAIKU_GAME_MODULE=$(wpath "$MODULE/game.dll"))"
  else
    mkdir -p "$RUN/data/code"
    cp "$MODULE/game.dll" "$MODULE/game.txt" "$RUN/data/code/"
  fi
else
  echo "no module in $MODULE yet: dev/recomp.sh $NAME builds it"
fi
echo "built $RUN/Haiku.exe"
