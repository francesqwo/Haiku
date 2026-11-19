/* tones: Corrin's speaking style (the Japanese game's talk tones) with a
 * Japanese-script translation on. See zakuro-core/src/tones.rs and
 * dev/reports/tones.md.
 *
 * The style is the byte at unit::Edit+0x2D (padding on the US build): 0, 1,
 * 2 for the talk tones _PC?1, _PC?2, _PC?3. sunglasses.c keeps it in saves.
 *
 * - unit::Edit::SetVoiceType (0x004195D8), which the avatar creator's Voice
 *   step calls (ChangedSelect's last case, a tail call): while a
 *   translation with speaking styles is on, the voice picked also picks
 *   its style, as the Japanese game's voices did (voice n: style n).
 * - Mess::FileLoad (0x003D1EAC) and Mess::FileLoadRoute (0x003D1874): before
 *   the game loads a message archive, the host makes the text layer read
 *   the archive in Corrin's style.
 *
 * Without a translation (haiku_tones_active 0) each override just calls the
 * game's function.
 */
#include <stdint.h>
#include "overrides.h"

typedef uint32_t (*HaikuTonesHook)(Context *ctx, uint32_t op, uint32_t a, uint32_t b);
/* set by Haiku's main before the game's code runs */
RECOMP_EXPORT HaikuTonesHook haiku_tones_hook = 0;
/* 1 while a translation with speaking styles is on (the host sets it) */
RECOMP_EXPORT volatile uint32_t haiku_tones_active = 0;

/* the same numbers as zakuro_core::tones::op */
enum { TN_SYNC = 1 };

#define EDIT_TONE 0x2Du

/* jpn: the Japanese game has the speaking styles itself (TalkUtil::GetTalkTone
   reads the tone from the voice, unit::Edit+0x2A, through its own table), so
   this layer stays off there: the host never sets haiku_tones_active on a
   Japanese build, and the voice override doesn't touch +0x2D there either */
int haiku_code_is_jpn(void); /* eur_region.c */

/* a: the archive's name (for HAIKU_TONES_TRACE) */
static void tones_sync(Context *ctx) {
    uint32_t saved[4] = {ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3]};
    haiku_tones_hook(ctx, TN_SYNC, saved[0], 0);
    ctx->r[0] = saved[0]; ctx->r[1] = saved[1]; ctx->r[2] = saved[2]; ctx->r[3] = saved[3];
}

/* Mess::FileLoad(const char*) */
RECOMP_OVERRIDE(0x003D1EAC) {
    if (haiku_tones_active && haiku_tones_hook)
        tones_sync(ctx);
    CALL(RECOMP_ORIGINAL(0x003D1EAC));
}

/* Mess::FileLoadRoute(const char*) */
RECOMP_OVERRIDE(0x003D1874) {
    if (haiku_tones_active && haiku_tones_hook)
        tones_sync(ctx);
    CALL(RECOMP_ORIGINAL(0x003D1874));
}

/* unit::Edit::SetVoiceType(int): r0 the edit, r1 the voice (0 to 2) */
RECOMP_OVERRIDE(0x004195D8) {
    if (haiku_tones_active && !haiku_code_is_jpn()) {
        uint32_t voice = ctx->r[1];
        mem_write8(ctx, ctx->r[0] + EDIT_TONE, (uint8_t)(voice < 3 ? voice : 0));
    }
    CALL(RECOMP_ORIGINAL(0x004195D8));
}
