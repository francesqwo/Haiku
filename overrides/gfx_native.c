/* gfx: the native renderer's entry points in the game.
 *
 * The engine draws every screen through prim::Context: Graphics::Render
 * picks a display (SetTargetDisplay), clears it (FrameBuffer::Clear), draws
 * that display's sorted prim::State list (prim::Context::RenderTarget, whose
 * states either carry 2D geometry or a callback that draws 3D scenes,
 * effects and captures), and copies the result to a display buffer
 * (DisplayBuffer::Transfer). These overrides hand each step to the native
 * renderer (Rust crate fates-gfx, source/gfx), which fills fates_gfx_hooks
 * at startup. With the hooks unset (FATES_NATIVE_GFX unset) every override
 * just runs the original.
 *
 * prim_target returns how many leading states the emulated path should
 * still draw: the native renderer takes the rest, so the emulated
 * RenderTarget is run on a truncated list. Addresses: USA SE v2.
 */
#include <stdint.h>
#include "overrides.h"

typedef struct FatesGfxHooks {
    /* prim::Context::RenderTarget(prim, disp). < 0: emulate all states. */
    int32_t (*prim_target)(Context *ctx, uint32_t prim, uint32_t disp);
    /* FrameBuffer::Clear(split), for the display last targeted. */
    void (*clear)(Context *ctx);
    /* SetTargetDisplay(display): 0x400 upper, 0x410 upper right, else lower. */
    void (*target_display)(Context *ctx, uint32_t display);
    /* DisplayBuffer::Transfer(displaybuffer object). */
    void (*transfer)(Context *ctx, uint32_t displaybuffer);
    /* Graphics::Render entry, once per rendered frame. */
    void (*frame)(Context *ctx);
    /* after the original RenderTarget returned (the native 3D path puts
       back the state list it handed over compacted) */
    void (*prim_target_done)(Context *ctx, uint32_t prim, uint32_t disp);
} FatesGfxHooks;

FatesGfxHooks fates_gfx_hooks;

/* hooks: after the original RenderTarget returned, the list's end back,
 * then the native side told (words: prim, disp, list, end) */
static void prim_target_end(Context *ctx, const uint32_t *w) {
    mem_write32(ctx, w[2] + 4, w[3]);
    if (fates_gfx_hooks.prim_target_done) fates_gfx_hooks.prim_target_done(ctx, w[0], w[1]);
}

/* prim::Context::RenderTarget(DispType::Type) */
RECOMP_OVERRIDE(0x0011BDF0) {
    uint32_t prim = ctx->r[0], disp = ctx->r[1];
    int32_t keep = fates_gfx_hooks.prim_target ? fates_gfx_hooks.prim_target(ctx, prim, disp) : -1;
    if (keep < 0) {
        CALL(RECOMP_ORIGINAL(0x0011BDF0));
        return;
    }
    /* the display's state list is a std::vector<prim::State*> at
       prim + 0x1878 + disp * 12 (begin, end, capacity). The emulated
       RenderTarget sees only the first `keep`; prim::Reset empties the list
       at the end of the frame, so the end is restored only when the original
       returns here (it can also be resumed elsewhere after a budget stop). */
    uint32_t list = prim + 0x1878 + disp * 12;
    uint32_t begin = mem_read32(ctx, list), end = mem_read32(ctx, list + 4);
    mem_write32(ctx, list + 4, begin + (uint32_t)keep * 4);
    /* hooks: a tail (recomp.h, CALL_THEN), so the end goes back and the
     * native side hears of it even when the original left for the host */
    CALL_THEN(RECOMP_ORIGINAL(0x0011BDF0), prim_target_end, prim, disp, list, end);
}

/* FrameBuffer::Clear(bool) */
RECOMP_OVERRIDE(0x00106554) {
    if (fates_gfx_hooks.clear) fates_gfx_hooks.clear(ctx);
    CALL(RECOMP_ORIGINAL(0x00106554));
}

/* SetTargetDisplay(unsigned int) */
RECOMP_OVERRIDE(0x001063C4) {
    if (fates_gfx_hooks.target_display) fates_gfx_hooks.target_display(ctx, ctx->r[0]);
    CALL(RECOMP_ORIGINAL(0x001063C4));
}

/* DisplayBuffer::Transfer(void) */
RECOMP_OVERRIDE(0x00106D50) {
    if (fates_gfx_hooks.transfer) fates_gfx_hooks.transfer(ctx, ctx->r[0]);
    CALL(RECOMP_ORIGINAL(0x00106D50));
}

/* called from the Graphics::Render override in gfx_survey.c */
void gfx_native_frame(Context *ctx) {
    if (fates_gfx_hooks.frame) fates_gfx_hooks.frame(ctx);
}

/* ---- 3D: Model::DrawMesh(RenderState&, TranslucencyLayer, const MeshData&,
 * const MTX34&) (0x0041DA20). The native renderer is told each mesh draw
 * (the Model, the RenderState, the layer, the h3d MeshData and the world
 * matrix) before the original builds its commands. A nonzero return means
 * the native renderer drew it and the original is skipped. */
typedef struct FatesGfxHooks3D {
    int32_t (*draw_mesh)(Context *ctx, uint32_t model, uint32_t state, uint32_t layer, uint32_t mesh, uint32_t world);
} FatesGfxHooks3D;

FatesGfxHooks3D fates_gfx_hooks_3d;

/* Model::DrawMesh */
RECOMP_OVERRIDE(0x0041DA20) {
    if (fates_gfx_hooks_3d.draw_mesh) {
        uint32_t world = mem_read32(ctx, ctx->r[13]);
        if (fates_gfx_hooks_3d.draw_mesh(ctx, ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3], world)) {
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x0041DA20));
}
