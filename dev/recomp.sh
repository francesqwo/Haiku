#!/usr/bin/env bash
# oneexe: build the game's code module for this checkout's overrides into
# $HAIKU_WORK/recomp-<name>/module (game.dll and game.txt, what a player's
# data/code holds; incremental: only C whose text changed is compiled
# again). dev/build.sh <name> then puts it in runs/<name>/data/code.
#
# Haiku's overrides are compiled into Haiku.exe itself (source/build.rs),
# so a change inside an override needs dev/build.sh only. The module has to
# be built again when the set of overrides' addresses or named functions
# changes, or the recompiler does (Haiku.exe says so at start: "the game's
# module was built for another version of Haiku").
#
# It builds Haiku.exe first (the module is made by Haiku.exe's own
# `--make-module`, the code players' PCs run), then runs it on the dump.
# Paths: dev/env.sh (HAIKU_WORK, HAIKU_DUMP, W64DEVKIT, ...). REGION_DUMP=<dump
# or installed data folder> builds from another game (EUR, JPN) into
# recomp-<name>-<tag> (TAG=<tag>, default the region's).
# overrides/amiibo_icons.c is made from the dump: run Haiku.exe's
# --apply-derived into this checkout first (README, Building from source).
# Usage (from a checkout or worktree root): dev/recomp.sh <name>
set -euo pipefail
NAME=${1:?usage: dev/recomp.sh <name>}
HERE=$(cd "$(dirname "$0")/.." && pwd)
. "$HERE/dev/env.sh"
SRC=${REGION_DUMP:-$DUMP}
[ -e "$SRC" ] || { echo "no dump at $SRC (set HAIKU_DUMP to your decrypted Fates-USA.cxi)"; exit 1; }
[ -f "$HERE/overrides/amiibo_icons.c" ] || { echo "overrides/amiibo_icons.c is missing: make the derived files first (Haiku.exe --apply-derived)"; exit 1; }
export CARGO_TARGET_DIR=$(wpath "$WORK/target-$NAME")
PROFILE=${PROFILE:-release}
cargo build --manifest-path "$(wpath "$HERE/source/Cargo.toml")" --profile "$PROFILE" ${CARGO_EXTRA:-}
R="$WORK/recomp-$NAME${REGION_DUMP:+-${TAG:-region}}"
mkdir -p "$R/module"
cp "$HERE/overrides/amiibo_icons.c" "$R/module/amiibo_icons.c"
# the compiler the module is built with: w64devkit's GCC, as the release's
export HAIKU_SETUP_TOOLCHAIN=$(wpath "$DEVKIT")
export HAIKU_SETUP_JOBS=${HAIKU_SETUP_JOBS:-8}
"$WORK/target-$NAME/$PROFILE/Haiku.exe" --make-module "$(wpath "$SRC")" "$(wpath "$R/module")"
echo "built the module into $R/module"
