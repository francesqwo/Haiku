/* gfx-gaps: map unit sprites, marks and HP gauges for the native renderer.
 *
 * UnitIcon::BatchCallback::OnBegin draws every unit on a battle map from a
 * prim callback state inside the scene's command mode, through its own
 * Pica:: helper calls (see source/gfx/src/unit_icon.rs). These overrides
 * tell the native renderer when it starts and ends, and hand it the calls
 * it makes that gfx_post.c does not observe already, through
 * gfx_post_observe (a no-op unless something is being recorded). The
 * originals always run. PicaHelper::DrawQuad's observe call is in
 * gfx_survey.c (one override per address). Addresses: USA SE v2. */
#include <stdint.h>
#include "overrides.h"

void gfx_post_observe(Context *ctx, uint32_t addr); /* gfx_post.c */

typedef struct FatesGfxHooksUnitIcon {
    /* UnitIcon::BatchCallback::OnBegin(this), before it runs */
    void (*begin)(Context *ctx, uint32_t callback);
    /* the same, after it returned */
    void (*end)(Context *ctx, uint32_t callback);
} FatesGfxHooksUnitIcon;

FatesGfxHooksUnitIcon fates_gfx_hooks_unit_icon;

/* hooks: what follows the original is a tail (recomp.h, CALL_THEN), run
 * when the original returns even if it left for the host on the way */
static void unit_icon_batch_end(Context *ctx, const uint32_t *w) {
    if (fates_gfx_hooks_unit_icon.end) fates_gfx_hooks_unit_icon.end(ctx, w[0]);
}

/* UnitIcon::BatchCallback::OnBegin(void) */
RECOMP_OVERRIDE(0x004F4200) {
    uint32_t self = ctx->r[0];
    if (fates_gfx_hooks_unit_icon.begin) fates_gfx_hooks_unit_icon.begin(ctx, self);
    CALL_THEN(RECOMP_ORIGINAL(0x004F4200), unit_icon_batch_end, self);
}

/* Pica::UniformMatrix(unsigned int, const nn::math::MTX34*) */
RECOMP_OVERRIDE(0x0013BE5C) {
    gfx_post_observe(ctx, 0x0013BE5C);
    CALL(RECOMP_ORIGINAL(0x0013BE5C));
}

/* PicaCommand::Transfer(void) */
RECOMP_OVERRIDE(0x0011F8B4) {
    gfx_post_observe(ctx, 0x0011F8B4);
    CALL(RECOMP_ORIGINAL(0x0011F8B4));
}

/* PicaHelper::SetVertexArray(unsigned int) */
RECOMP_OVERRIDE(0x0017DB88) {
    gfx_post_observe(ctx, 0x0017DB88);
    CALL(RECOMP_ORIGINAL(0x0017DB88));
}

/* the UnitIcon functions that replay a PicaCommand of combiner stages
 * (unit_icon.rs reads the command at their entry: their own
 * PicaCommand::Transfer can be a tail branch, which the generated code
 * runs in place without the override above) */

/* UnitIcon::TransferTexture(const Color8&, const Color8&) */
RECOMP_OVERRIDE(0x004F4674) {
    gfx_post_observe(ctx, 0x004F4674);
    CALL(RECOMP_ORIGINAL(0x004F4674));
}

/* UnitIcon::TransferTextureGray(const Color8&, const Color8&) */
RECOMP_OVERRIDE(0x004F48F4) {
    gfx_post_observe(ctx, 0x004F48F4);
    CALL(RECOMP_ORIGINAL(0x004F48F4));
}

/* UnitIcon::TransferTextureSilhouette(const Color8&) */
RECOMP_OVERRIDE(0x004F4E08) {
    gfx_post_observe(ctx, 0x004F4E08);
    CALL(RECOMP_ORIGINAL(0x004F4E08));
}

/* UnitIcon::BatchCallback::DrawMark(UnitIcon::BatchDrawInfo*) */
RECOMP_OVERRIDE(0x004F43C0) {
    gfx_post_observe(ctx, 0x004F43C0);
    CALL(RECOMP_ORIGINAL(0x004F43C0));
}

/* UnitIcon::BatchCallback::DrawHPGauge(UnitIcon::BatchDrawInfo*) */
RECOMP_OVERRIDE(0x004F3AC0) {
    gfx_post_observe(ctx, 0x004F3AC0);
    CALL(RECOMP_ORIGINAL(0x004F3AC0));
}
