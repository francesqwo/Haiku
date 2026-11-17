/* gfx-final: unit-icon textures rendered natively.
 *
 * UnitIconInstance::OnBegin, a prim callback, makes each new unit icon's
 * texture on the GPU: UnitIconObject::CaptureTexture points the framebuffer
 * at the icon's own RGBA4 texture and draws the unit sheet into it
 * (CaptureSheet, DrawSprite, DrawQuad, VertexBuffer::RenderQuad). These
 * overrides tell the native renderer (source/gfx/src/capture.rs) when
 * OnBegin and each CaptureTexture start and end, and hand it the calls it
 * needs that no other override observes (through gfx_post_observe, a no-op
 * unless something is being recorded). The native renderer draws the same
 * quads and writes the texture into guest memory, where everything that
 * samples it reads it. The originals always run.
 * Addresses: USA SE v2. */
#include <stdint.h>
#include "overrides.h"

void gfx_post_observe(Context *ctx, uint32_t addr); /* gfx_post.c */

typedef struct FatesGfxHooksCapture {
    /* UnitIconInstance::OnBegin(this): before (0) and after (1) */
    void (*instance)(Context *ctx, uint32_t self, uint32_t after);
    /* UnitIconObject::CaptureTexture(object, sheet type): before / after */
    void (*capture)(Context *ctx, uint32_t object, uint32_t type, uint32_t after);
} FatesGfxHooksCapture;

FatesGfxHooksCapture fates_gfx_hooks_capture;

/* hooks: what follows the original is a tail (recomp.h, CALL_THEN), run
 * when the original returns even if it left for the host on the way */
static void capture_instance_end(Context *ctx, const uint32_t *w) {
    if (fates_gfx_hooks_capture.instance) fates_gfx_hooks_capture.instance(ctx, w[0], 1);
}

static void capture_texture_end(Context *ctx, const uint32_t *w) {
    if (fates_gfx_hooks_capture.capture) fates_gfx_hooks_capture.capture(ctx, w[0], w[1], 1);
}

/* UnitIconInstance::OnBegin(void) */
RECOMP_OVERRIDE(0x001F23D0) {
    uint32_t self = ctx->r[0];
    if (fates_gfx_hooks_capture.instance) fates_gfx_hooks_capture.instance(ctx, self, 0);
    CALL_THEN(RECOMP_ORIGINAL(0x001F23D0), capture_instance_end, self);
}

/* UnitIconObject::CaptureTexture(UnitSheet::Type) */
RECOMP_OVERRIDE(0x001D6650) {
    uint32_t object = ctx->r[0], type = ctx->r[1];
    if (fates_gfx_hooks_capture.capture) fates_gfx_hooks_capture.capture(ctx, object, type, 0);
    CALL_THEN(RECOMP_ORIGINAL(0x001D6650), capture_texture_end, object, type);
}

/* ShaderObj::SetViewProj(const nn::math::MTX44&) const */
RECOMP_OVERRIDE(0x001376AC) {
    gfx_post_observe(ctx, 0x001376AC);
    CALL(RECOMP_ORIGINAL(0x001376AC));
}

/* ShaderObj::SetTexMtx(const nn::math::MTX34&) const (its
 * Pica::UniformMatrix is a tail branch) */
RECOMP_OVERRIDE(0x00138848) {
    gfx_post_observe(ctx, 0x00138848);
    CALL(RECOMP_ORIGINAL(0x00138848));
}

/* PicaHelper::SetTexture(const ITexture*) (null: texture unit 0 off; its
 * Pica::TextureSamplerType is a tail branch) */
RECOMP_OVERRIDE(0x0017DA98) {
    gfx_post_observe(ctx, 0x0017DA98);
    CALL(RECOMP_ORIGINAL(0x0017DA98));
}
