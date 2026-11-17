/* hfr: game timing (USA Special Edition v2).
 *
 * Fates' clock is GameTime at 0x006DCA84 (see vendor/zakuro/crates/zakuro/src/hfr.rs
 * for the layout). nnMain's loop is one game tick: input, Proc::Exec (logic,
 * scene and animation updates), Graphics::Render, the buffer swap, and
 * Graphics::WaitVSync, which waits for `step` VBlanks and sets
 * delta = clamp(vsyncs elapsed, step, slow). About 200 Tick functions advance
 * their timers, cameras, faces, cursors and fades by that delta, so the game
 * keeps real-time speed at either tick rate.
 *
 * GameTime::ResetFrameStep(n) sets step = slow = delta = n. Scenes pass 2
 * (30 ticks a second) for battles (BattleUtil::BattleInitialize), 3D cutscenes
 * (CutSceneMain), the title screen (Title::Create), Live2D (ProcLive2D),
 * class changes (ClassChangeSequence::CreateAction) and castle unit moves
 * (castle::Sequence::TickMoveUnit); the map, menus and castle pass 1.
 *
 * With fates_hfr_max_step = 1 those 30 Hz scenes tick at 60 Hz with delta 1:
 * twice the ticks, each advancing half as far. That keeps the title screen's
 * and cutscenes' timing (within a few frames), but battles end about 17%
 * sooner (723 instead of 870 refreshes for the prologue's first battle):
 * some battle waits count ticks rather than delta. So the default, 0, keeps
 * the game's own pacing; pc.game_60fps turns the limit on, as an experiment.
 * The host sets it from the settings; it takes effect at the next scene
 * change (the next ResetFrameStep).
 */
#include "overrides.h"

/* set by the host: the most VBlanks a tick may wait for, 0 for no limit. */
uint32_t fates_hfr_max_step = 0;
/* read by the host: the step the current scene asked for. */
uint32_t fates_hfr_scene_step = 1;

/* GameTime::ResetFrameStep(unsigned int) */
RECOMP_OVERRIDE(0x004EBB38) {
    uint32_t want = ctx->r[0];
    uint32_t limit = *(volatile uint32_t *)&fates_hfr_max_step;
    fates_hfr_scene_step = want;
    if (limit && want > limit)
        ctx->r[0] = limit;
    CALL(RECOMP_ORIGINAL(0x004EBB38));
}
