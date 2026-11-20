#!/usr/bin/env bash
# builder: makes the release folder (what the zip holds), and the zip.
#
#   dev/builder/package.sh <out dir>
#
# oneexe: the release is one program. <out dir>/Haiku/ gets:
#   Haiku.exe              the game: the engine, Haiku's own overrides and
#                          Haiku Setup, built here (ship profile), with no
#                          game code: the game's code is recompiled on the
#                          player's PC into data/code/game.dll, which
#                          Haiku.exe builds and loads itself
#   SDL3.dll               beside it
#   README.txt, LICENSES/  what it is, and every licence of what is bundled
#   setup/w64devkit/       the part of w64devkit's GCC 16.2 the module build uses
#   setup/derived.pack     the recipes for Haiku's game-derived files
#   setup/derived-eur.pack, derived-jpn.pack   the same for the EUR and
#                          Japanese games
#   setup/assets/          Haiku's own assets that hold no game data
#   setup/payload.txt      the release's commit, date and version
# and no game code or data; then <out dir>/Haiku.zip. (Up to 1.1 the release
# shipped Haiku Setup.exe, the engine as objects and the overrides' C, and
# linked Haiku.exe on the player's PC.) The region maps the overrides are
# bound with are inside Haiku.exe (builder/src/regions.rs).
#
# Paths come from dev/env.sh (HAIKU_WORK, HAIKU_TOOLCHAIN, W64DEVKIT, ...).
# Also:
#   SDL3_DLL=<file>  the official SDL3 x64 release DLL (default dist-dll/SDL3.dll).
#   REPO_URL=<url>   the project's page, named in LICENSES/Toolchain.txt as
#                    where the compiler's source is offered.
#   REMAKE_DERIVED=1 [ROMFS=<dir>]  make dev/builder/derived.pack again from
#                    the repository's game-derived files (only a checkout
#                    that has them) and a RomFS extracted from the dump
#                    (default $HAIKU_MAIN/data/romfs). Without it, the
#                    committed dev/builder/derived.pack is what ships.
#   ONLY=<step>      runs one step: exe, toolchain, setup, derived, rest, zip.
#   HAIKU_RELEASE_VERSION=<version>  updater: the version this release is,
#                    instead of source/VERSION's (the one place it lives:
#                    bump that file for a release). Haiku.exe reports it,
#                    and its updater compares it with GitHub's latest release;
#                    payload.txt names it too, which the updater checks a
#                    downloaded release against.
# After packaging: dev/release/sign.sh <out dir>/Haiku.zip (Haiku.zip.sig,
# which the release must carry for Haiku's in-game updater to install it).
set -euo pipefail
OUT=${1:?usage: dev/builder/package.sh <out dir>}
HERE=$(cd "$(dirname "$0")/../.." && pwd)
. "$HERE/dev/env.sh"
REPO_URL=${REPO_URL:-https://github.com/tyle-beep/Haiku}
SDL3_DLL=${SDL3_DLL:-$HERE/dist-dll/SDL3.dll}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
REL="$OUT/Haiku"
SETUP="$REL/setup"
mkdir -p "$SETUP"
step() { [ -z "${ONLY:-}" ] || [ "$ONLY" = "$1" ]; }
# updater: the release's version, built into Haiku.exe (source/build.rs)
VERSION=${HAIKU_RELEASE_VERSION:-$(tr -d '
 ' < "$HERE/source/VERSION")}
case "$VERSION" in ''|*[!A-Za-z0-9.+-]*) echo "bad version: '$VERSION' (source/VERSION or HAIKU_RELEASE_VERSION)"; exit 1;; esac
export HAIKU_RELEASE_VERSION=$VERSION
echo "version: $VERSION"
# the builds the release ships hold no path of this machine: the source,
# cargo's registry and the toolchain are renamed in panic messages
REMAP="--remap-path-prefix=$(wpath "$HERE")=haiku --remap-path-prefix=$(wpath "$T/cargo")=cargo --remap-path-prefix=$(wpath "$T/rustup")=rustup --remap-path-prefix=$(wpath "$WORK")=work"
# rustc names the import stubs of raw-dylib crates (windows-rs) after the
# folder it builds in, and those names stay in the exe's symbols: the
# release builds go through a junction with a neutral name in Windows'
# shared Public folder
NEUTRAL_U="$(upath "${PUBLIC:?PUBLIC (the shared Public folder) is not set}")/haiku-build"
NEUTRAL=$(wpath "$NEUTRAL_U")
[ -e "$NEUTRAL_U" ] || cmd //c mklink //J "$NEUTRAL" "$(wpath "$WORK")" > /dev/null
trap 'cmd //c rmdir "$NEUTRAL" > /dev/null 2>&1' EXIT
REMAP="$REMAP --remap-path-prefix=$NEUTRAL=work"

if step exe; then
  # oneexe: Haiku.exe itself, with Haiku's overrides compiled in by
  # source/build.rs (gcc from w64devkit, the build folder's path mapped
  # away) and no game code
  EXE_TARGET=${EXE_TARGET:-target-release-exe}
  export CARGO_TARGET_DIR="$NEUTRAL\\$EXE_TARGET"
  LOG="$WORK/release-exe.log"
  RUSTFLAGS="$REMAP" cargo build --manifest-path "$(wpath "$HERE/source/Cargo.toml")" --profile ship --locked --offline > "$LOG" 2>&1 || { tail -20 "$LOG"; exit 1; }
  cp "$WORK/$EXE_TARGET/ship/Haiku.exe" "$REL/Haiku.exe"
  # the old release's programs never ship again
  rm -f "$REL/Haiku Setup.exe"
  rm -rf "$SETUP/engine" "$SETUP/overrides" "$SETUP/eur" "$SETUP/jpn"
  echo "exe: $(du -h "$REL/Haiku.exe" | cut -f1)"
fi

if step toolchain; then
  # the files of w64devkit the module build opens (dev/builder/toolchain.txt,
  # from a traced build), with their licences
  [ "$(tr -d '\r\n' < "$DEVKIT/VERSION.txt" 2> /dev/null)" = 2.11.0 ] || { echo "$DEVKIT is not w64devkit 2.11.0, the version LICENSES/Toolchain.txt offers the source of"; exit 1; }
  TC="$SETUP/w64devkit"
  rm -rf "$TC"; mkdir -p "$TC"
  while IFS= read -r f; do
    [ -z "$f" ] || [ "${f:0:1}" = "#" ] && continue
    mkdir -p "$TC/$(dirname "$f")"; cp "$DEVKIT/$f" "$TC/$f"
  done < "$HERE/dev/builder/toolchain.txt"
  echo "toolchain: $(find "$TC" -type f | wc -l) files, $(du -sh "$TC" | cut -f1)"
fi

if step setup; then
  # Haiku's own assets: the HD sunglasses layer (drawn by Haiku) and the
  # pipeline keys
  rm -rf "$SETUP/assets"; mkdir -p "$SETUP/assets/builtin"
  cp -r "$HERE/assets/builtin/textures" "$SETUP/assets/builtin/"
  cp -r "$HERE/assets/pipelines" "$SETUP/assets/"
  [ -f "$SDL3_DLL" ] || { echo "no SDL3.dll at $SDL3_DLL: put the official SDL3 x64 release's SDL3.dll in dist-dll/ (or set SDL3_DLL)"; exit 1; }
  # oneexe: beside Haiku.exe, where it loads it from
  cp "$SDL3_DLL" "$REL/SDL3.dll"
  rm -f "$SETUP/SDL3.dll"
  # updater: the version third (the in-game updater checks it)
  echo "$(git -C "$HERE" rev-parse --short HEAD) $(date -u +%Y-%m-%d) $VERSION" > "$SETUP/payload.txt"
fi

if step derived; then
  PACK="$HERE/dev/builder/derived.pack"
  if [ "${REMAKE_DERIVED:-0}" = 1 ]; then
    # the recipes made again from the repository's own derived files (only
    # a checkout that has them: they are made from the game) and the RomFS
    ROMFS=${ROMFS:-$MAIN/data/romfs}
    "$REL/Haiku.exe" --make-derived "$(wpath "$HERE")" "$(wpath "$ROMFS")" "$(wpath "$PACK")" "$(wpath "$WORK/release-derived-report.tsv")"
  fi
  [ -f "$PACK" ] || { echo "no $PACK"; exit 1; }
  cp "$PACK" "$SETUP/derived.pack"
  # eur2: the EUR game's recipes (Haiku's text in its five languages, its
  # button prompt sheets); REMAKE_DERIVED_EUR=1 makes them again from
  # EUR_FILES (dev/eur/text/make_text.py's and dev/eur/glyphs.py's output,
  # default WORK/eur-files) and EUR_ROMFS
  EPACK="$HERE/dev/builder/derived-eur.pack"
  if [ "${REMAKE_DERIVED_EUR:-0}" = 1 ]; then
    "$REL/Haiku.exe" --make-derived-eur "$(wpath "${EUR_FILES:-$WORK/eur-files}")" "$(wpath "${EUR_ROMFS:?set EUR_ROMFS to the EUR RomFS}")" "$(wpath "$EPACK")" "$(wpath "$WORK/release-derived-eur-report.tsv")"
  fi
  if [ -f "$EPACK" ]; then cp "$EPACK" "$SETUP/derived-eur.pack"; else rm -f "$SETUP/derived-eur.pack"; fi
  # jpprep: the Japanese game's (Haiku's text in Japanese), made with
  # REMAKE_DERIVED_JPN=1 from JPN_FILES (dev/eur/text/make_text.py's J
  # output, default WORK/jpn-files) and JPN_ROMFS
  JPACK="$HERE/dev/builder/derived-jpn.pack"
  if [ "${REMAKE_DERIVED_JPN:-0}" = 1 ]; then
    "$REL/Haiku.exe" --make-derived-jpn "$(wpath "${JPN_FILES:-$WORK/jpn-files}")" "$(wpath "${JPN_ROMFS:?set JPN_ROMFS to the Japanese RomFS}")" "$(wpath "$JPACK")"
  fi
  if [ -f "$JPACK" ]; then cp "$JPACK" "$SETUP/derived-jpn.pack"; else rm -f "$SETUP/derived-jpn.pack"; fi
fi

if step rest; then
  rm -rf "$REL/LICENSES"; mkdir -p "$REL/LICENSES"
  cp "$HERE/ZAKURO-LICENSE.txt" "$REL/LICENSES/Zakuro-MIT.txt"
  cp "$HERE/dist-dll/SDL3-LICENSE.txt" "$REL/LICENSES/SDL3-zlib.txt"
  cp "$HERE/vendor/3dsrecomp/LICENSE" "$REL/LICENSES/3dsrecomp-MIT.txt"
  cp "$HERE/vendor/zakuro/crates/zakuro/data/gamecontrollerdb-LICENSE.txt" "$REL/LICENSES/SDL_GameControllerDB-zlib.txt"
  cp "$HERE/dev/builder/LICENSES/"* "$REL/LICENSES/"
  cp "$DEVKIT/COPYING.MinGW-w64-runtime.txt" "$REL/LICENSES/MinGW-w64-runtime.txt"
  cp "$HERE/dev/builder/README.txt" "$REL/README.txt"
  # the place the compiler's source is offered (GPL-3.0 section 6(d))
  sed -i "s|@REPO_URL@|$REPO_URL|g" "$REL/LICENSES/Toolchain.txt" "$REL/README.txt"
  # the Rust crates in Haiku.exe, and their licences
  python -I "$HERE/dev/builder/crates.py" "$HERE" > "$REL/LICENSES/Rust-crates.txt"
  python -I "$HERE/dev/builder/crates.py" "$HERE" --texts > "$REL/LICENSES/Rust-crates-licences.txt"
  # the release ships the game only: no mods, not even the examples (the
  # user's call, 2026-11-16)
  rm -rf "$REL/mods-examples" "$REL/mods"
  echo "release folder: $(find "$REL" -type f | wc -l) files, $(du -sh "$REL" | cut -f1)"
fi

if step zip; then
  # Haiku.zip with the folder Haiku/ at its top, entries in sorted order
  rm -f "$OUT/Haiku.zip"
  python -I - "$OUT" <<'PY'
import os, sys, zipfile
out = sys.argv[1]
files = []
for root, dirs, names in os.walk(os.path.join(out, "Haiku")):
    for n in names:
        files.append(os.path.join(root, n))
files.sort(key=lambda p: os.path.relpath(p, out).replace(os.sep, "/"))
with zipfile.ZipFile(os.path.join(out, "Haiku.zip"), "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for f in files:
        z.write(f, os.path.relpath(f, out).replace(os.sep, "/"))
print(f"Haiku.zip: {len(files)} files, {os.path.getsize(os.path.join(out, 'Haiku.zip'))} bytes")
PY
  (cd "$OUT" && sha256sum Haiku.zip > Haiku.zip.sha256 && cat Haiku.zip.sha256)
fi
