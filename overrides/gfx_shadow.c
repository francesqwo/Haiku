/* gfx-shadow: ShadowScene and the CommandAlter layer, for the native
 * renderer (fates-gfx, source/gfx/src/shadow.rs and command_alter.rs).
 *
 * ShadowScene (FieldShadow's shadow pass, in the lower display's prim list)
 * binds its own framebuffer, draws the shadow casters through
 * Model::DrawMesh (already hooked in gfx_native.c) and blurs the result with
 * ShadowGauss::Execute. These overrides tell the native renderer where the
 * pass begins and ends, and let it take the blur.
 *
 * CommandAlter collects rewrites of the command list the upper display just
 * built (projection and world matrices, light attitude, camera callbacks);
 * CommandBuffer::Copy runs them on the copy of that range it makes for the
 * right eye. The overrides only report them to the native layer.
 *
 * With the hooks unset (FATES_NATIVE_GFX unset) every override just runs the
 * original. Addresses: USA SE v2.
 */
#include <stdint.h>
#include "overrides.h"

typedef struct FatesGfxHooksShadow {
    /* ShadowScene::OnBegin(this): before and after the original */
    void (*scene_begin)(Context *ctx, uint32_t scene);
    void (*scene_end)(Context *ctx, uint32_t scene);
    /* ShadowGauss::Execute(this). Nonzero: the native renderer blurs the
       shadow map, the original builds no commands. */
    int32_t (*gauss)(Context *ctx, uint32_t gauss);
    /* AlterImpl::AddBuffer(impl, data, bytes, type, command offset), before
       the original copies the data */
    void (*alter_buffer)(Context *ctx, uint32_t data, uint32_t bytes, uint32_t type, uint32_t offset);
    /* CommandAlter::AddCallback(callback) */
    void (*alter_callback)(Context *ctx, uint32_t callback);
    /* CommandAlter::Clear(), before the upper display is built */
    void (*alter_clear)(Context *ctx);
    /* CommandAlter::Execute(copy, offset, delta): the right eye's copy */
    void (*alter_execute)(Context *ctx, uint32_t copy, uint32_t offset);
} FatesGfxHooksShadow;

FatesGfxHooksShadow fates_gfx_hooks_shadow;

/* hooks: what follows the original is a tail (recomp.h, CALL_THEN), run
 * when the original returns even if it left for the host on the way */
static void shadow_scene_end(Context *ctx, const uint32_t *w) {
    if (fates_gfx_hooks_shadow.scene_end) fates_gfx_hooks_shadow.scene_end(ctx, w[0]);
}

/* ShadowScene::OnBegin(void) */
RECOMP_OVERRIDE(0x001A4D10) {
    uint32_t scene = ctx->r[0];
    if (fates_gfx_hooks_shadow.scene_begin) fates_gfx_hooks_shadow.scene_begin(ctx, scene);
    CALL_THEN(RECOMP_ORIGINAL(0x001A4D10), shadow_scene_end, scene);
}

/* ShadowGauss::Execute(void) */
RECOMP_OVERRIDE(0x00203F90) {
    if (fates_gfx_hooks_shadow.gauss && fates_gfx_hooks_shadow.gauss(ctx, ctx->r[0])) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x00203F90));
}

/* AlterImpl::AddBuffer(const void*, int, AlterConst::Type, unsigned int) */
RECOMP_OVERRIDE(0x0013C1A0) {
    if (fates_gfx_hooks_shadow.alter_buffer)
        fates_gfx_hooks_shadow.alter_buffer(ctx, ctx->r[1], ctx->r[2], ctx->r[3] & 0xFFFF, mem_read32(ctx, ctx->r[13]));
    CALL(RECOMP_ORIGINAL(0x0013C1A0));
}

/* CommandAlter::AddCallback(CommandAlter::Callback*) */
/* gfx-post: observes the same calls; defined in gfx_post.c */
void gfx_post_observe(Context *ctx, uint32_t addr);

RECOMP_OVERRIDE(0x001AC484) {
    gfx_post_observe(ctx, 0x001AC484);
    if (fates_gfx_hooks_shadow.alter_callback) fates_gfx_hooks_shadow.alter_callback(ctx, ctx->r[0]);
    CALL(RECOMP_ORIGINAL(0x001AC484));
}

/* CommandAlter::Clear(void) */
RECOMP_OVERRIDE(0x00106640) {
    if (fates_gfx_hooks_shadow.alter_clear) fates_gfx_hooks_shadow.alter_clear(ctx);
    CALL(RECOMP_ORIGINAL(0x00106640));
}

/* CommandAlter::Execute(unsigned int, unsigned int, unsigned int) */
RECOMP_OVERRIDE(0x0010B924) {
    gfx_post_observe(ctx, 0x0010B924);
    if (fates_gfx_hooks_shadow.alter_execute) fates_gfx_hooks_shadow.alter_execute(ctx, ctx->r[0], ctx->r[1]);
    CALL(RECOMP_ORIGINAL(0x0010B924));
}
