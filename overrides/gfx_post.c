/* gfx-post: the native post-effect layer's entry points in the game.
 *
 * Post effects (BloomContext, ShadowGauss, StealthCaptureNode, ...) are not
 * drawn through prim states: they call the engine's PostEffectContext API
 * (InitBeforeDraw, TextureAssign, DrawQuads), the Pica:: register helpers
 * directly (TexEnv*, Viewport, Blend, TextureSetting...), nn::gr's
 * FrameBuffer::MakeCommand to pick a render target, and nngx copies
 * (FilterBlockImage, AddBlockImageCopyCommand, AddMemoryFillCommand), some
 * of them from CommandAlter callbacks.
 *
 * While the native renderer records a post effect (Rust crate fates-gfx,
 * source/gfx/src/post.rs, which sets `recording`), every one of those calls
 * is handed to it with its arguments, in call order, which is the order
 * the GPU would run them. The originals always run too, so the emulated GPU
 * state stays what the game expects. With the hooks unset (FATES_NATIVE_GFX
 * unset) nothing here does anything but run the original.
 *
 * Some of the addresses are also overridden by gfx_survey.c (call
 * counters) and widescreen.c (Pica::Viewport/ScissorTest); those overrides
 * call gfx_post_observe themselves, since an address has one override.
 * Addresses: USA SE v2. */
#include <stdint.h>
#include "overrides.h"

typedef struct FatesGfxPostHooks {
    /* nonzero while a post effect is being recorded */
    volatile int32_t recording;
    /* a post-effect island starts / ends (the prim::Callback object) */
    void (*begin)(Context *ctx, uint32_t callback);
    void (*end)(Context *ctx, uint32_t callback);
    /* a recorded call: the function's address and its arguments */
    void (*call)(Context *ctx, uint32_t function, const uint32_t *args, uint32_t count);
} FatesGfxPostHooks;

FatesGfxPostHooks fates_gfx_hooks_post;

/* how many word arguments each observed function takes (r0-r3, then the
 * guest stack) */
static uint32_t post_arg_count(uint32_t addr) {
    switch (addr) {
    case 0x00126640: return 7;  /* Pica::TexEnvSrc */
    case 0x0012FFA8: return 7;  /* Pica::TexEnvOperand */
    case 0x0012FF68: return 3;  /* Pica::TexEnvCombine */
    case 0x003D2214: return 5;  /* Pica::TexEnvConst */
    case 0x00126060: return 3;  /* Pica::TexEnvScale */
    case 0x00130098: return 8;  /* Pica::TexEnvBufferInput */
    case 0x001300F0: return 6;  /* Pica::TextureSamplerType */
    case 0x00137718: return 14; /* Pica::TextureSetting */
    case 0x001263E0: return 4;  /* Pica::Viewport */
    case 0x00125F60: return 5;  /* Pica::ScissorTest */
    case 0x0013027C: return 1;  /* Pica::Blend */
    case 0x001303FC: return 6;  /* Pica::BlendFunc */
    case 0x00126548: return 3;  /* Pica::AlphaTest */
    case 0x0012657C: return 3;  /* Pica::DepthTest */
    case 0x001302D8: return 1;  /* Pica::Culling */
    case 0x003D26D0: return 4;  /* Pica::ColorMask */
    case 0x001376BC: return 2;  /* Pica::UniformMatrix(MTX44) */
    case 0x001301A8: return 1;  /* Pica::Draw */
    case 0x00204030: return 3;  /* PostEffectContext::DrawQuads */
    case 0x00203BF8: return 3;  /* PostEffectContext::InitBeforeDraw */
    case 0x00113354: return 4;  /* nn::gr::CTR::FrameBuffer::MakeCommand */
    case 0x0010653C: return 2;  /* FrameBuffer::MakeCommand (the display's) */
    case 0x005C85EC: return 5;  /* nngxFilterBlockImage */
    case 0x005C854C: return 7;  /* nngxAddBlockImageCopyCommand */
    case 0x00113C00: return 8;  /* nngxAddMemoryFillCommand */
    case 0x001AC484: return 1;  /* CommandAlter::AddCallback */
    case 0x0010B924: return 3;  /* CommandAlter::Execute */
    default: return 4;
    }
}

/* called at the entry of each observed function, before the original */
void gfx_post_observe(Context *ctx, uint32_t addr) {
    if (!fates_gfx_hooks_post.recording || !fates_gfx_hooks_post.call) return;
    uint32_t args[16];
    uint32_t n = post_arg_count(addr);
    for (uint32_t i = 0; i < n; i++)
        args[i] = i < 4 ? ctx->r[i] : mem_read32(ctx, ctx->r[13] + 4 * (i - 4));
    fates_gfx_hooks_post.call(ctx, addr, args, n);
}

/* the observed functions no other override file takes: each hands its
 * call over, then runs the game's own code */
/* Pica::TexEnvSrc */
RECOMP_OVERRIDE(0x00126640) {
    gfx_post_observe(ctx, 0x00126640);
    CALL(RECOMP_ORIGINAL(0x00126640));
}

/* Pica::TexEnvOperand */
RECOMP_OVERRIDE(0x0012FFA8) {
    gfx_post_observe(ctx, 0x0012FFA8);
    CALL(RECOMP_ORIGINAL(0x0012FFA8));
}

/* Pica::TexEnvCombine */
RECOMP_OVERRIDE(0x0012FF68) {
    gfx_post_observe(ctx, 0x0012FF68);
    CALL(RECOMP_ORIGINAL(0x0012FF68));
}

/* Pica::TexEnvConst */
RECOMP_OVERRIDE(0x003D2214) {
    gfx_post_observe(ctx, 0x003D2214);
    CALL(RECOMP_ORIGINAL(0x003D2214));
}

/* Pica::TexEnvScale */
RECOMP_OVERRIDE(0x00126060) {
    gfx_post_observe(ctx, 0x00126060);
    CALL(RECOMP_ORIGINAL(0x00126060));
}

/* Pica::TexEnvBufferInput */
RECOMP_OVERRIDE(0x00130098) {
    gfx_post_observe(ctx, 0x00130098);
    CALL(RECOMP_ORIGINAL(0x00130098));
}

/* Pica::TextureSamplerType */
RECOMP_OVERRIDE(0x001300F0) {
    gfx_post_observe(ctx, 0x001300F0);
    CALL(RECOMP_ORIGINAL(0x001300F0));
}

/* Pica::BlendFunc */
RECOMP_OVERRIDE(0x001303FC) {
    gfx_post_observe(ctx, 0x001303FC);
    CALL(RECOMP_ORIGINAL(0x001303FC));
}

/* Pica::AlphaTest */
RECOMP_OVERRIDE(0x00126548) {
    gfx_post_observe(ctx, 0x00126548);
    CALL(RECOMP_ORIGINAL(0x00126548));
}

/* Pica::Culling */
RECOMP_OVERRIDE(0x001302D8) {
    gfx_post_observe(ctx, 0x001302D8);
    CALL(RECOMP_ORIGINAL(0x001302D8));
}

/* Pica::ColorMask */
RECOMP_OVERRIDE(0x003D26D0) {
    gfx_post_observe(ctx, 0x003D26D0);
    CALL(RECOMP_ORIGINAL(0x003D26D0));
}

/* Pica::UniformMatrix(MTX44) */
RECOMP_OVERRIDE(0x001376BC) {
    gfx_post_observe(ctx, 0x001376BC);
    CALL(RECOMP_ORIGINAL(0x001376BC));
}

/* PostEffectContext::InitBeforeDraw */
RECOMP_OVERRIDE(0x00203BF8) {
    gfx_post_observe(ctx, 0x00203BF8);
    CALL(RECOMP_ORIGINAL(0x00203BF8));
}

/* nn::gr::CTR::FrameBuffer::MakeCommand */
RECOMP_OVERRIDE(0x00113354) {
    gfx_post_observe(ctx, 0x00113354);
    CALL(RECOMP_ORIGINAL(0x00113354));
}

/* FrameBuffer::MakeCommand(unsigned*, bool): the display's framebuffer.
 * It ends in a branch into nn::gr's MakeCommand, which the generated code
 * runs in place, so that one's override does not see this call. */
RECOMP_OVERRIDE(0x0010653C) {
    gfx_post_observe(ctx, 0x0010653C);
    CALL(RECOMP_ORIGINAL(0x0010653C));
}

/* CommandAlter::AddCallback */
/* 0x001AC484: overridden in gfx_shadow.c, which calls gfx_post_observe (one override per address) */

/* CommandAlter::Execute */
/* 0x0010B924: overridden in gfx_shadow.c, which calls gfx_post_observe (one override per address) */


/* PrimPostEffectCallback::OnBegin: the post effect's object runs its Draw
 * (BloomContext::Draw for the avatar creator's bloom). The native renderer
 * records everything it does in between. */
/* hooks: what follows the original is a tail (recomp.h, CALL_THEN), run
 * when the original returns even if it left for the host on the way */
static void post_effect_end(Context *ctx, const uint32_t *w) {
    if (fates_gfx_hooks_post.end) fates_gfx_hooks_post.end(ctx, w[0]);
}

RECOMP_OVERRIDE(0x0021E220) {
    uint32_t callback = ctx->r[0];
    if (fates_gfx_hooks_post.begin) fates_gfx_hooks_post.begin(ctx, callback);
    CALL_THEN(RECOMP_ORIGINAL(0x0021E220), post_effect_end, callback);
}
