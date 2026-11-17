#!/usr/bin/env bash
# builder: the Corresponding Source of the GPL-covered compiler the release
# bundles (GCC 16.2.0 and Binutils 2.47 from w64devkit 2.11.0, with GMP,
# MPFR and MPC linked into cc1, and the MinGW-w64 v14.0.0 runtime), as the
# zip offered beside Haiku.zip on the release page (LICENSES/Toolchain.txt,
# GPL-3.0 section 6(d)).
#
#   dev/builder/toolchain-source.sh [<sources dir>] [<out dir>]
#
# <sources dir> (default $HAIKU_WORK/toolchain-src) holds the upstream
# tarballs w64devkit 2.11.0's Dockerfile fetches, the w64devkit 2.11.0
# source and SHA256SUMS; each is checked against the SHA-256 below (the
# Dockerfile's own) before anything is written. Writes
# <out dir>/Haiku-toolchain-source-w64devkit-2.11.0.zip (default out dir
# $HAIKU_WORK/haiku-release) and its .sha256.
set -euo pipefail
HERE=$(cd "$(dirname "$0")/../.." && pwd)
. "$HERE/dev/env.sh"
SRC=$(upath "${1:-$WORK/toolchain-src}")
OUT=$(upath "${2:-$WORK/haiku-release}")
NAME=Haiku-toolchain-source-w64devkit-2.11.0
# the SHA-256 of each file: the upstream tarballs as w64devkit 2.11.0's
# Dockerfile checks them, and the w64devkit 2.11.0 source this zip offers
EXPECT="e6738e29597f733270731aa90600f37ffdc045079dfc27ec7e8192cc81085c3e  gcc-16.2.0.tar.xz
154ab23b60070e8f27013c22977f1129425d67d1e8acd6e13010e617811e4cff  binutils-2.47.tar.xz
a3c2b80201b89e68616f4ad30bc66aee4927c3ce50e33929ca819d5c43538898  gmp-6.3.0.tar.xz
b67ba0383ef7e8a8563734e2e889ef5ec3c3b898a01d00fa0a6869ad81c6ce01  mpfr-4.2.2.tar.xz
91204cd32f164bd3b7c992d4a6a8ce6519511aadab30f78b6982d0bf8d73e931  mpc-1.4.1.tar.xz
6eaf921d9eb987d3820b364ea9775bc19b965ec81490b6fdd716526c28e1995c  mingw-w64-v14.0.0.tar.bz2
d24cfcf20ae5a344fd005e66e64ec642f702eecec320d47d02d31410b0c83572  w64devkit-2.11.0-src.tar.gz"

missing=0
while read -r sum file; do
  if [ ! -f "$SRC/$file" ]; then echo "missing: $SRC/$file"; missing=1; continue; fi
  got=$(sha256sum "$SRC/$file" | cut -d' ' -f1)
  if [ "$got" != "$sum" ]; then echo "WRONG: $file is $got, expected $sum"; exit 1; fi
  echo "ok  $file"
done <<< "$EXPECT"
[ $missing = 0 ] || { echo "the sources are incomplete; nothing written"; exit 1; }
# the integrator's SHA256SUMS must agree too
if [ -f "$SRC/SHA256SUMS" ]; then
  while read -r sum file; do
    [ -n "$sum" ] || continue
    grep -qx "$sum  ${file#\*}" <<< "$EXPECT" || { echo "SHA256SUMS disagrees: $sum $file"; exit 1; }
  done < <(tr -d '\r' < "$SRC/SHA256SUMS")
  echo "ok  SHA256SUMS"
fi
# the Dockerfile in the w64devkit source names exactly these versions and hashes
DOCKER=$(python -I -c 'import sys, tarfile; print(tarfile.open(sys.argv[1]).extractfile("w64devkit-2.11.0/Dockerfile").read().decode())' "$SRC/w64devkit-2.11.0-src.tar.gz")
for v in "GCC_VERSION=16.2.0" "BINUTILS_VERSION=2.47" "GMP_VERSION=6.3.0" "MPFR_VERSION=4.2.2" "MPC_VERSION=1.4.1" "MINGW_VERSION=14.0.0"; do
  grep -q "$v" <<< "$DOCKER" || { echo "the w64devkit Dockerfile doesn't say $v"; exit 1; }
done
while read -r sum file; do
  case $file in w64devkit-*) continue;; esac
  grep -q "$sum" <<< "$DOCKER" || { echo "the w64devkit Dockerfile doesn't name $file's hash"; exit 1; }
done <<< "$EXPECT"
echo "ok  the Dockerfile names every version and hash"

STAGE="$WORK/toolchain-source-stage/$NAME"
rm -rf "$WORK/toolchain-source-stage"; mkdir -p "$STAGE"
while read -r sum file; do cp "$SRC/$file" "$STAGE/"; done <<< "$EXPECT"
printf '%s\n' "$EXPECT" > "$STAGE/SHA256SUMS"
cat > "$STAGE/README.txt" <<'TXT'
Source code of the compiler bundled with Haiku
==============================================

Haiku's release (Haiku.zip) bundles, in Haiku\setup\w64devkit, a part of
w64devkit 2.11.0 (x64, https://github.com/skeeto/w64devkit), unchanged:
GCC 16.2.0 and GNU Binutils 2.47 (GNU GPL version 3), whose cc1.exe has
GMP 6.3.0, MPFR 4.2.2 and MPC 1.4.1 (GNU LGPL version 3) linked in
statically, and the MinGW-w64 v14.0.0 runtime and headers. This zip is
their Corresponding Source, offered under section 6(d) of the GPL version
3 beside Haiku.zip on the same release page. Haiku's LICENSES\Toolchain.txt
says the same.

The files
---------

gcc-16.2.0.tar.xz          GCC 16.2.0, the GNU Compiler Collection
                           (gcc, cc1, collect2, lto-wrapper, libgcc).
binutils-2.47.tar.xz       GNU Binutils 2.47 (as, ld, ar).
gmp-6.3.0.tar.xz           GMP 6.3.0, linked into cc1.
mpfr-4.2.2.tar.xz          MPFR 4.2.2, linked into cc1.
mpc-1.4.1.tar.xz           MPC 1.4.1, linked into cc1.
mingw-w64-v14.0.0.tar.bz2  MinGW-w64 v14.0.0: the Windows headers, the C
                           runtime's startup code and import libraries,
                           libmingwex, winpthreads.
w64devkit-2.11.0-src.tar.gz
                           w64devkit 2.11.0's own source: its Dockerfile,
                           which is the complete build script, and the
                           patch series it applies (src/binutils, src/gcc,
                           src/gcc-final, src/mingw).
SHA256SUMS                 the SHA-256 of each. The upstream tarballs'
                           are the ones w64devkit's Dockerfile checks when
                           it downloads them.

They are the upstream releases, unmodified: gcc, binutils, gmp, mpfr and
mpc from the GNU project (https://ftp.gnu.org/gnu/), mingw-w64 from
https://sourceforge.net/projects/mingw-w64/.

How w64devkit builds them
-------------------------

Unpack w64devkit-2.11.0-src.tar.gz and build its Dockerfile, as its
README describes:

    docker build -t w64devkit .
    docker run --rm w64devkit >w64devkit-x64.exe

The Dockerfile downloads these same tarballs, checking each against the
SHA-256 in SHA256SUMS, unpacks them, applies each patch series with quilt
in the order of its series file, and configures and builds, in order:
Binutils and GCC as a cross compiler, the MinGW-w64 headers, CRT and
winpthreads, then Binutils, GMP, MPFR, MPC and GCC again as native Windows
programs (x86_64-w64-mingw32). The result is the w64devkit 2.11.0 release
whose files Haiku bundles.

Licences: GCC and Binutils are under the GNU GPL version 3 (COPYING3 in
their tarballs), GMP, MPFR and MPC under the GNU LGPL version 3,
MinGW-w64's runtime under the licences in its COPYING files, and
w64devkit under the Unlicense.
TXT
rm -f "$OUT/$NAME.zip"
mkdir -p "$OUT"
# stored, not deflated: the tarballs are compressed already
python -I - "$WORK/toolchain-source-stage" "$OUT/$NAME.zip" <<'PY'
import os, sys, zipfile
stage, out = sys.argv[1], sys.argv[2]
files = sorted(os.path.join(r, n) for r, d, ns in os.walk(stage) for n in ns)
with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as z:
    for f in files:
        z.write(f, os.path.relpath(f, stage).replace(os.sep, "/"))
print(f"{out}: {len(files)} files, {os.path.getsize(out)} bytes")
PY
rm -rf "$WORK/toolchain-source-stage"
(cd "$OUT" && sha256sum "$NAME.zip" > "$NAME.zip.sha256" && cat "$NAME.zip.sha256")
