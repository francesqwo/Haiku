# Where the dev scripts find the work folder, the toolchain and the dump.
# Sourced by dev/build.sh, dev/recomp.sh and dev/builder/*.sh; every path
# comes from the environment or from where this checkout is, none is fixed.
#
#   HAIKU_WORK       build folders, recompiled code and logs. Default: the
#                    folder that holds toolchain/ two levels up (a worktree
#                    at work/wt/<name>), else ../../work beside a checkout,
#                    else work/ inside this checkout (ignored by git).
#   HAIKU_TOOLCHAIN  holds rustup/, cargo/ and w64devkit/ (default
#                    $HAIKU_WORK/toolchain). A part that isn't there comes
#                    from PATH, as installed normally.
#   W64DEVKIT        w64devkit 2.11.0 (default $HAIKU_TOOLCHAIN/w64devkit).
#   HAIKU_MAIN       the checkout holding game/ (the dump) and data/ (the
#                    installed RomFS). Default: the repository's main
#                    worktree, which is this checkout when there is one.
#   HAIKU_DUMP       the decrypted dump the recompiler reads (default
#                    $HAIKU_MAIN/game/Fates-USA.cxi).
#   CARGO_BUILD_JOBS default 4.
HERE=${HERE:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
upath() { if command -v cygpath > /dev/null; then cygpath -u "$1"; else printf '%s\n' "$1"; fi; }
wpath() { if command -v cygpath > /dev/null; then cygpath -w "$1"; else printf '%s\n' "$1"; fi; }
if [ -z "${HAIKU_WORK:-}" ]; then
  if [ -d "$HERE/../../toolchain" ]; then HAIKU_WORK=$(cd "$HERE/../.." && pwd)
  elif [ -d "$HERE/../../work/toolchain" ]; then HAIKU_WORK=$(cd "$HERE/../../work" && pwd)
  else HAIKU_WORK="$HERE/work"; mkdir -p "$HAIKU_WORK"
  fi
fi
WORK=$(upath "$HAIKU_WORK")
T=$(upath "${HAIKU_TOOLCHAIN:-$WORK/toolchain}")
DEVKIT=$(upath "${W64DEVKIT:-$T/w64devkit}")
if [ -z "${HAIKU_MAIN:-}" ]; then
  HAIKU_MAIN=$(git -C "$HERE" worktree list --porcelain 2> /dev/null | sed -n '1s/^worktree //p')
  [ -n "$HAIKU_MAIN" ] || HAIKU_MAIN=$HERE
fi
MAIN=$(upath "$HAIKU_MAIN")
DUMP=$(upath "${HAIKU_DUMP:-$MAIN/game/Fates-USA.cxi}")
[ -d "$T/rustup" ] && export RUSTUP_HOME=$(wpath "$T/rustup")
[ -d "$T/cargo" ] && { export CARGO_HOME=$(wpath "$T/cargo"); export PATH="$T/cargo/bin:$PATH"; }
[ -d "$DEVKIT/bin" ] && export PATH="$DEVKIT/bin:$PATH"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
