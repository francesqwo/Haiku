/* transblur: the battle transitions' zoom blur, as an option.
 *
 * When a fight starts from the map (and when it ends), the battle camera is
 * a TransitionCamera (SubjectCamera for the way in, ComebackCamera /
 * SilentComebackCamera for the way out). Each tick TransitionCamera::Tick
 * (0x001F1A60) puts its curves into the post effect parameters at
 * 0x00700914:
 *   +9    the radial blur's strength (the way in: 0 -> 0xAA -> 0xAA -> 0,
 *         TransitionCamera::CreateEncountCurve), from the curve at +0x14
 *         (keys counted at +0x1C);
 *   +8    how far the blurred copy is zoomed (0x14), +10 its jitter (0);
 *   +0xC  the bloom's strength (a spline at +0x3C/+0x40): the glow.
 * BloomContext::Draw (0x001AB0BC) then, with +9 set, ends the frame with
 * BloomContext::SetFinalStateWithRadialBlur(true) (0x001AAAA0): the final
 * pass mixes in the copy of the frame before (RadialBlur::Capture, kept in
 * a surface), zoomed about the centre, at +9/255. That is a feedback
 * effect: each picture holds the trail of all the ones before it. A
 * picture in between can't be drawn for such a frame (fates-gfx hfr.rs
 * `carries`), so the whole zoom was shown at the transition's 30 pictures a
 * second, cross-faded (smooth2), however high the display's rate.
 *
 * With Battle Transition Blur off (`haiku_transition_blur` 0, set by the
 * host from the setting) the final pass is set up as the game sets it up
 * on any frame without the blur, SetFinalStateWithRadialBlur(false), while
 * a TransitionCamera drives the blur. Nothing else changes: the camera's
 * path, the zoom, the bloom's glow, the fades and every word of game state
 * (+9 itself, which the map's 2D and BattleController::IsBlurring read,
 * keeps the game's value). The frame then reads nothing an earlier frame
 * left, so it gets pictures in between like any other: the zoom glides at
 * the display's rate. Other radial blurs (cutscenes' RadialBlur command,
 * the boss camera) are untouched.
 *
 * A TransitionCamera drives the blur when its Tick ran with blur keys
 * (+0x1C > 0) since the last few BloomContext draws: the transitions tick
 * at 30 a second, so a tick is followed by one or two draws; four is the
 * limit, after which the camera has gone (its Tick stopped) and the blur
 * is the game's again.
 *
 * FATES_TRANSBLUR_LOG=1 logs each transition's first and last suppressed
 * frame. Addresses: USA SE v2. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include "overrides.h"

#define BLUR_PARAMS 0x00700914u
#define DRAWS_PER_TICK 4u

/* the host's: 1 the game's own blur, 0 none in battle transitions */
uint32_t haiku_transition_blur = 1;
/* how many final passes were set up without the blur (for the run's summary) */
uint32_t haiku_transition_unblurred;

static struct {
    uint32_t camera; /* the TransitionCamera that ticked with blur keys */
    uint32_t draws;  /* BloomContext draws since */
    int run;         /* frames suppressed in a row, for the log */
} tb = { 0, DRAWS_PER_TICK, 0 };

static int logging(void) {
    static int v = -1;
    if (v < 0) {
        const char *e = getenv("FATES_TRANSBLUR_LOG");
        v = e && *e && *e != '0';
    }
    return v;
}

/* TransitionCamera::Tick(void): note a blur curve at work (before the
 * original, which only reads it) */
RECOMP_OVERRIDE(0x001F1A60) {
    uint32_t self = ctx->r[0];
    if ((int32_t)mem_read32(ctx, self + 0x1C) > 0) {
        tb.camera = self;
        tb.draws = 0;
    }
    CALL(RECOMP_ORIGINAL(0x001F1A60));
}

/* BloomContext::SetFinalStateWithRadialBlur(bool) */
RECOMP_OVERRIDE(0x001AAAA0) {
    int ours = tb.draws < DRAWS_PER_TICK;
    if (tb.draws < DRAWS_PER_TICK) tb.draws++;
    if (ctx->r[1] && ours && !haiku_transition_blur) {
        ctx->r[1] = 0;
        haiku_transition_unblurred++;
        if (logging() && tb.run++ == 0)
            fprintf(stderr, "[transblur] camera %08X: transition without the radial blur (strength %u)\n", tb.camera, mem_read8(ctx, BLUR_PARAMS + 9));
    } else if (tb.run) {
        if (logging()) fprintf(stderr, "[transblur] camera %08X: %d frames drawn without the radial blur\n", tb.camera, tb.run);
        tb.run = 0;
    }
    CALL(RECOMP_ORIGINAL(0x001AAAA0));
}
