/* jpn: Live2D drawn by the native renderer (source/gfx/src/live2d.rs).
 *
 * My Room's Live2D views (the visit's close-up in every build; the petting
 * minigame, スキンシップ, of the Japanese one) draw a Cubism 2 model through
 * its 3DS backend: Live2DCallBack::OnBegin, a prim callback on the lower
 * screen, runs Live2DModelManager::Draw, and the model's
 * live2d::DrawParam_3DS::drawTexture(texNo, indexCount, vertexCount,
 * indices, xys, uvs, opacity, colorComposition) draws each of its meshes
 * with nn::gd: GPU commands the pure build has no GPU for. With the native
 * renderer on, this override hands each mesh to the host instead (the model
 * matrix, MTX44, is DrawParam_3DS+0x1C, set by setupDraw's caller; its
 * textures are nn::gd Texture2D at +0x7C), and the original is not run.
 * Without the hook (native renderer off) the original runs.
 * Addresses: USA SE v2 (the same backend in every build). */
#include <stdint.h>
#include "overrides.h"

typedef struct FatesGfxHooksLive2D {
    /* DrawParam_3DS::drawTexture: nonzero when the host drew it */
    uint32_t (*draw)(Context *ctx, uint32_t self, uint32_t tex, uint32_t index_count, uint32_t vertex_count,
                     uint32_t indices, uint32_t xys, uint32_t uvs, uint32_t opacity_bits, uint32_t composition);
} FatesGfxHooksLive2D;

FatesGfxHooksLive2D fates_gfx_hooks_live2d;

/* live2d::DrawParam_3DS::drawTexture(int, int, int, unsigned short*,
   float*, float*, float, int): r0 this, r1 texNo, r2 indexCount, r3
   vertexCount, the stack: indices, xys, uvs, colorCompositionType; s0 the
   opacity */
RECOMP_OVERRIDE(0x004A4504) {
    if (fates_gfx_hooks_live2d.draw) {
        uint32_t sp = ctx->r[13];
        uint32_t opacity = ctx->vfp[0]; /* s0, the float's bits */
        if (fates_gfx_hooks_live2d.draw(ctx, ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3], mem_read32(ctx, sp),
                                        mem_read32(ctx, sp + 4), mem_read32(ctx, sp + 8), opacity, mem_read32(ctx, sp + 12))) {
            RETURN_TO(ctx->r[14]);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x004A4504));
}
