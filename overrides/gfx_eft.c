/* gfx-eft: nw::eft particles for the native renderer.
 *
 * Every particle and stripe of a 3D scene is drawn by EffectEmitter::Draw
 * (0x001C76DC, called from EffectNode::OnDrawXlu inside SceneSystem's
 * scene pass), which hands the RenderState's command pointer (+0x10) to
 * nw::eft::System::MakeDrawEmitterSetCommand (0x0031E7BC). In nw::eft for
 * CTR the particle simulation and the draw encoders are one pass:
 * EmitterCalc::CalcParticle steps each particle and has DrawCommander
 * write its values (position, scale, rotation, colours, texture offsets)
 * as uniform blocks, with the emitter's shader and material blobs and the
 * common quad kicks. So the game's code always runs; these overrides tell
 * the native renderer (fates-gfx eft.rs) which RenderState the emitter set
 * is drawn for and which words it wrote. A nonzero return from the hook
 * means the native renderer drew them, and the words are taken back out of
 * the command list (the pointer returned is the one the call was given),
 * so the emulated GPU never sees them. With the hook unset (the default)
 * both overrides only run the original. Addresses: USA SE v2. */
#include <stdint.h>
#include "overrides.h"

typedef struct FatesGfxHooksEft {
    /* after MakeDrawEmitterSetCommand(system, cmd, set, ...) wrote the
       words [from, to); state is the RenderState of the EffectEmitter::Draw
       it runs in, or 0 when unknown. Nonzero: drop the words. */
    int32_t (*draw)(Context *ctx, uint32_t state, uint32_t system, uint32_t set, uint32_t from, uint32_t to);
    /* Pica::FogConfig(on, flip, r, g, b): the fog registers the fixed
       state (prim::Context::SetupFixedState: the 2D layer, the map's
       sprites, gauges and cursor inside a scene pass) leaves on, which an
       effect drawn after it inherits; mesh materials' fog the native side
       ports itself */
    void (*fog)(Context *ctx, uint32_t on, uint32_t flip, uint32_t r, uint32_t g, uint32_t b);
} FatesGfxHooksEft;

FatesGfxHooksEft fates_gfx_hooks_eft;

static uint32_t eft_render_state;

/* hooks: what follows the original is a tail (recomp.h, CALL_THEN), run
 * when the original returns even if it left for the host on the way */
static void eft_draw_end(Context *ctx, const uint32_t *w) {
    (void)ctx;
    eft_render_state = w[0];
}

/* words: state, system, set, from; r0 is the pointer after the words */
static void eft_command_end(Context *ctx, const uint32_t *w) {
    uint32_t from = w[3], to = ctx->r[0];
    if (to > from && fates_gfx_hooks_eft.draw(ctx, w[0], w[1], w[2], from, to)) ctx->r[0] = from;
}

/* EffectEmitter::Draw(RenderState&, float) */
RECOMP_OVERRIDE(0x001C76DC) {
    if (!fates_gfx_hooks_eft.draw) {
        CALL(RECOMP_ORIGINAL(0x001C76DC));
        return;
    }
    uint32_t saved = eft_render_state;
    eft_render_state = ctx->r[1];
    CALL_THEN(RECOMP_ORIGINAL(0x001C76DC), eft_draw_end, saved);
}

/* nw::eft::System::MakeDrawEmitterSetCommand(unsigned int*, EmitterSet*,
   bool, nn::math::VEC3*, float) -> the command pointer after its words */
RECOMP_OVERRIDE(0x0031E7BC) {
    if (!fates_gfx_hooks_eft.draw) {
        CALL(RECOMP_ORIGINAL(0x0031E7BC));
        return;
    }
    uint32_t sys = ctx->r[0], from = ctx->r[1], set = ctx->r[2];
    uint32_t state = eft_render_state;
    /* a RenderState left over from a call a budget stop resumed elsewhere */
    if (state && mem_read32(ctx, state + 0x10) != from) state = 0;
    CALL_THEN(RECOMP_ORIGINAL(0x0031E7BC), eft_command_end, state, sys, set, from);
}

/* Pica::FogConfig(bool, bool, unsigned char, unsigned char, unsigned char) */
RECOMP_OVERRIDE(0x0011B850) {
    if (fates_gfx_hooks_eft.fog)
        fates_gfx_hooks_eft.fog(ctx, ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3], mem_read32(ctx, ctx->r[13]));
    CALL(RECOMP_ORIGINAL(0x0011B850));
}
