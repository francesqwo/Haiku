/* gfx-terrain: 2D drawn inside a 3D scene pass, for the native renderer.
 *
 * Battle maps draw unit sprites, HP gauges, icons, panels and the cursor
 * from a scene's node callbacks through the prim layer's command mode:
 * prim::Context::BeginCommand(RenderState&) marks where the state vector
 * ends, prim calls append states, and prim::Context::EndCommand renders
 * the states appended since into the scene's command list. These
 * overrides tell the native renderer (fates-gfx scene2d.rs) which
 * RenderState the command mode draws for and when its states are
 * complete. The originals always run. With the hooks unset (the default)
 * they only run the original. Addresses: USA SE v2. */
#include <stdint.h>
#include "overrides.h"

typedef struct FatesGfxHooksScene2d {
    /* prim::Context::BeginCommand(context, RenderState&), before it runs */
    void (*begin)(Context *ctx, uint32_t prim_ctx, uint32_t render_state);
    /* prim::Context::EndCommand(context), before it runs */
    void (*end)(Context *ctx, uint32_t prim_ctx);
} FatesGfxHooksScene2d;

FatesGfxHooksScene2d fates_gfx_hooks_scene2d;

/* prim::Context::BeginCommand(RenderState&) */
RECOMP_OVERRIDE(0x00415F84) {
    if (fates_gfx_hooks_scene2d.begin) fates_gfx_hooks_scene2d.begin(ctx, ctx->r[0], ctx->r[1]);
    CALL(RECOMP_ORIGINAL(0x00415F84));
}

/* prim::Context::EndCommand(void) */
RECOMP_OVERRIDE(0x004159A4) {
    if (fates_gfx_hooks_scene2d.end) fates_gfx_hooks_scene2d.end(ctx, ctx->r[0]);
    CALL(RECOMP_ORIGINAL(0x004159A4));
}
