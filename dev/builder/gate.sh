#!/usr/bin/env bash
# builder: the regression gate on a Haiku that Haiku Setup built, in its own
# folder (data/ and assets/ made from the dump, no link to the repository's
# game/ or assets/): from boot at 5:3 and 21:9 (os-save3.rec, 9400 frames),
# pressA to 3000, and Astra (the cpu bench's f11000 state with s5.rec, run in
# the cpu folder, see below).
#   dev/builder/gate.sh <install dir> <tag>
# Output: work/builder-gate-<tag>-*.log and the summary on stdout. ONLY=astra
# runs Astra alone. HAIKU_RECS=<dir> holds os-save3.rec and pressA.rec
# (default dev/; the replays aren't in the public repository), and
# HAIKU_CPU_BENCH=<dir> the cpu bench folder (Astra is skipped without it).
set -u
DIR=${1:?usage: dev/builder/gate.sh <install dir> <tag>}
TAG=${2:-builder}
HERE=$(cd "$(dirname "$0")/../.." && pwd)
. "$HERE/dev/env.sh"
W=$WORK
RECS=$(upath "${HAIKU_RECS:-$HERE/dev}")
CPU=$(upath "${HAIKU_CPU_BENCH:-$W/wt/cpu/runs/cpu}")
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
# Astra: the cpu bench's state (f11000) and replay (s5). The state was saved
# in the cpu folder (its game/ ROM image, userdata and saves) and goes wrong
# anywhere else (main's own Haiku.exe too, checked), so this runs as
# dev/lilith/gate.sh does: the exe copied there; the log keeps its last 20 MB
L="$W/builder-gate-$TAG-astra.log"
[ -d "$CPU" ] || { echo "astra skipped: no cpu bench folder at $CPU"; exit 0; }
cp "$DIR/Haiku.exe" "$CPU/Haiku-$TAG.exe"
(cd "$CPU" && FATES_LOAD=st/f11000.state ZAKURO_REPLAY=rec/s5.rec timeout 300 "./Haiku-$TAG.exe" --headless 13500 --mute 2>&1 | tail -c 20000000 > "$L"; echo "astra exit=${PIPESTATUS[0]} $(grep -o 'interpreted 0' "$L" | head -1) $(grep -o 'unavailable [0-9]*' "$L" | tail -1) $(grep -o 'unanswered [0-9]*' "$L" | tail -1)")
rm -f "$CPU/Haiku-$TAG.exe"
