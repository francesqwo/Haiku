# Game-function overrides

C files here replace recompiled game functions (see vendor/3dsrecomp/docs/overrides.md).
One file per subsystem (e.g. `widescreen.c`, `timing.c`, `fs.c`) so branches merge cleanly.
Apply them with `dev/recomp.sh <name>`, then `dev/build.sh <name>` links that output.
Addresses are for USA Special Edition v2 (CTR-P-BFZE). The development notes name them from the game's own debug tables (research/symbols.tsv), which are not in the public repository because they come from the game.
