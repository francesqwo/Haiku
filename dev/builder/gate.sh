#!/usr/bin/env bash
# builder: the regression gate on a Haiku that Haiku Setup built, in its own
# folder (data/ and assets/ made from the dump, no link to the repository's
# game/ or assets/): from boot at 5:3 and 21:9 (os-save3.rec, 9400 frames),
# pressA to 3000, and Astra (WORK/bench/astra.sh).
#   dev/builder/gate.sh <install dir> <tag>
# Output: work/builder-gate-<tag>-*.log and the summary on stdout. ONLY=astra
# runs Astra alone. HAIKU_RECS=<dir> holds os-save3.rec and pressA.rec
# (default dev/; the replays aren't in the public repository).
set -u
DIR=${1:?usage: dev/builder/gate.sh <install dir> <tag>}
TAG=${2:-builder}
HERE=$(cd "$(dirname "$0")/../.." && pwd)
. "$HERE/dev/env.sh"
W=$WORK
RECS=$(upath "${HAIKU_RECS:-$HERE/dev}")
cd "$DIR"
[ -e game ] && { echo "the install has a game/ folder; the gate is for one without"; exit 1; }
unset FATES_ROOT ZAKURO_WIDE_ASPECT
boot() { # tag aspect
  local t=$1 m=$2
  rm -rf userdata/haiku/user
  if [ $m = wide ]; then export ZAKURO_WIDE_ASPECT=21:9; else unset ZAKURO_WIDE_ASPECT; fi
  ZAKURO_REPLAY="$(cygpath -w "$RECS/os-save3.rec")" ./Haiku.exe --headless 9400 --mute 2>&1 | tail -c 20000000 > "$W/builder-gate-$TAG-boot-$t.log"; local rc=${PIPESTATUS[0]}
  local sums=$(md5sum userdata/haiku/user/extdata/*/Chapter3.bak userdata/haiku/user/savedata/*/Rating 2>/dev/null | cut -c1-8 | tr '\n' ' ')
  local L="$W/builder-gate-$TAG-boot-$t.log"
  echo "boot-$t exit=$rc saves=[$sums] $(grep -o 'this run interpreted [0-9]*' "$L" | tail -1) $(grep -o 'late [0-9]*, dropped [0-9]*' "$L" | tail -1) $(grep -o 'unanswered [0-9]*' "$L" | tail -1) | $(grep -o 'nothing emulated ran' "$L" | tail -1)"
  unset ZAKURO_WIDE_ASPECT
}
if [ "${ONLY:-}" != astra ]; then
boot nat nat
boot wide wide
L="$W/builder-gate-$TAG-pressA.log"
ZAKURO_REPLAY="$(cygpath -w "$RECS/pressA.rec")" ./Haiku.exe --headless 3000 --mute 2>&1 | tail -c 20000000 > "$L"; echo "pressA exit=${PIPESTATUS[0]} $(grep -o 'unanswered [0-9]*' "$L" | tail -1) $(grep -o 'this run interpreted [0-9]*' "$L" | tail -1) | $(grep -o 'nothing emulated ran' "$L" | tail -1)"
fi
# Astra: oneexe: the bench's skill-activation segment (WORK/bench/astra.sh, the
# cpu bench it used to take is gone), the install's exe with its own module
# (HAIKU_GAME_MODULE: the bench copies the exe alone) and assets
L="$W/builder-gate-$TAG-astra.log"
[ -f "$W/bench/astra.sh" ] || { echo "astra skipped: no bench at $W/bench"; exit 0; }
# versions: an install with games\<version>\ plays the version Haiku.exe
# --versions says plays; a flat folder plays from itself
GAME="$DIR"
if [ -d "$DIR/games" ]; then GAME=$(cygpath -u "$("$DIR/Haiku.exe" --versions | grep '(plays)' | sed 's/.* in //' | tr -d '\r')"); fi
HAIKU_GAME_MODULE="$(wpath "$GAME/data/code/game.dll")" ASSETS="$GAME/assets" bash "$W/bench/astra.sh" "$DIR/Haiku.exe" | tee "$L"
