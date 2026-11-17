#!/usr/bin/env bash
# junction assets/ into a run folder: dev/link-assets.sh runs/<name>
HERE=$(cd "$(dirname "$0")/.." && pwd); RUN="$1"
[ -e "$RUN/assets" ] || cmd //c mklink //J "$(cygpath -w "$RUN/assets")" "$(cygpath -w "$HERE/assets")" >/dev/null
