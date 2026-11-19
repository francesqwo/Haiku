/* eur: the title this code is recompiled from, which Haiku reads at start-up
 * to know whose addresses the code has (zakuro_core::region). As written it
 * is the USA Special Edition; dev/eur/translate.py writes the EUR id in its
 * copy for the EUR build (the line marked "eur: title"). */
#include "overrides.h"

/* oneexe: Haiku.exe sets it to the game's module's title when it binds the
 * module, before any of the game's code runs (source/src/module.rs) */
unsigned long long haiku_code_title = 0x0004000000179800ull; /* eur: title */

/* jpprep: whether the code is a Japanese build's (Fire Emblem if: the
 * Special Edition 000400000012DE00, or the single-route 12DC00 and 12DD00
 * cartridges). The Japanese game has no Lang::Folder (it is Japanese only):
 * the overrides that pick a language ask this first (jpn_text.c). */
int haiku_code_is_jpn(void) {
    const unsigned id = (unsigned)(haiku_code_title >> 8) & 0xFFFFu;
    return (haiku_code_title >> 32) == 0x00040000u && (id == 0x12DEu || id == 0x12DCu || id == 0x12DDu);
}
