/* widescreen: the top screen drawn wider than the console's 5:3, the way a
 * native port would, in the game's own camera code.
 *
 * The host draws the top screen into a render target k times wider than
 * 400x240 (see zakuro-gpu/src/wide.rs), where k = aspect / (5/3). Here the
 * game is made to fill it without stretching:
 *
 *  - ICamera::UpdateProj (0x004DADD4) and FieldCamera::UpdateProj
 *    (0x001966AC) are written out natively. A top-screen camera gets a
 *    horizontal field of view k times wider: a perspective one divides its
 *    x scale by k, an off-axis frustum and an orthographic one widen their
 *    left and right planes about the centre. Everything the game derives
 *    from the projection follows by itself: the rotated and stereo matrices
 *    (UpdateStereo), the view-projection at +0x228/+0x268 and the culling
 *    frustum (UpdateFrustum builds its planes from the inverse of P*V, which
 *    ICamera::IsReject and BattleUnit::UpdateCameraFrustumVisibility test),
 *    so nothing at the new edges is culled.
 *  - An orthographic top-screen camera (the 2D layer's, SetupOrtho(400,240))
 *    keeps the original 400 units in the middle at their original size, so
 *    the UI keeps its proportions, centred.
 *  - ICamera::GetWorldToScreen (0x00544424), which places 2D things over 3D
 *    ones (map HP bars, balloons, icons), maps into that same widened 2D
 *    space.
 *
 * Fields of ICamera used here (USA Special Edition v2):
 *   +0x28 fovy (radians), +0x2C aspect, +0x30 near, +0x34 far,
 *   +0x41 projection type (0 perspective, 1 frustum, 2 orthographic),
 *   +0x58/+0x5C/+0x60/+0x64 left, right, bottom, top,
 *   +0xC8 projection (4x4, before the display rotation),
 *   +0x358 texture projection (3x4), +0x3F8 viewport (4 shorts x, y, w, h),
 *   +0x5B0 drawn to the upper screen (SetupOrtho sets it for DispType 0).
 */

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "overrides.h"

/* the host's top-screen aspect ratio, width over height, set by the host
 * (fates-pc main.rs) before the game runs. 0 or unset means the console's. */
float (*fates_wide_aspect_hook)(void) = 0;

/* wide2: the native renderer's note of a textured full-width rectangle
 * DrawRect stretched: the prim::State it went into, the quad's index in
 * it, the centre it was stretched about and by how much. Unset (the
 * emulated GPU), the stretch stays. */
void (*fates_wide2_rect_hook)(uint32_t state, uint32_t quad, float centre, float k) = 0;

#define NATIVE_ASPECT (400.0f / 240.0f)

static float wide_k(void) {
    static int env_read = 0;
    static float env_k = 0.0f;
    if (!env_read) {
        const char *v = getenv("FATES_WIDE_K");
        env_k = v ? (float)atof(v) : 0.0f;
        env_read = 1;
    }
    if (env_k > 0.0f) return env_k < 1.0f ? 1.0f : env_k;
    if (!fates_wide_aspect_hook) return 1.0f;
    float aspect = fates_wide_aspect_hook();
    if (!(aspect > NATIVE_ASPECT + 0.001f)) return 1.0f;
    return aspect / NATIVE_ASPECT;
}

static int wide_log(void) {
    static int state = -1;
    if (state < 0) state = getenv("FATES_WIDE_LOG") != 0;
    return state;
}

static float rf(Context *ctx, uint32_t a) { return vfp_from_s(mem_read32(ctx, a)); }
static void wf(Context *ctx, uint32_t a, float v) {
    uint32_t bits;
    memcpy(&bits, &v, 4);
    mem_write32(ctx, a, bits);
}

/* whether a camera draws the whole upper screen, which is the one made
 * wider: flagged for the upper screen (+0x5B0) with a viewport (+0x3F8)
 * across its full width. A camera drawing into part of the screen (a 3D
 * model in a window) keeps its projection; its viewport is moved with the
 * rest of the 2D layout instead, see Pica::Viewport below. */
static int top_camera(Context *ctx, uint32_t cam) {
    if (mem_read8(ctx, cam + 0x5B0) == 0) return 0;
    int16_t vx = (int16_t)mem_read16(ctx, cam + 0x3F8), vw = (int16_t)mem_read16(ctx, cam + 0x3FC);
    return vx <= 0 && vx + vw >= 400;
}

/* the camera's horizontal widening, 1 for any camera not drawing the top
 * screen. */
static float camera_k(Context *ctx, uint32_t cam) {
    float k = wide_k();
    if (k <= 1.0f) return 1.0f;
    return top_camera(ctx, cam) ? k : 1.0f;
}

/* ---- logging, FATES_WIDE_LOG=1: each camera once per projection ---- */
#define SEEN_MAX 256
static struct { uint32_t cam; uint32_t key; } seen[SEEN_MAX];
static int seen_count;

static void log_camera(Context *ctx, uint32_t cam, int type, float k, const char *who) {
    if (!wide_log()) return;
    uint32_t key = (uint32_t)type | mem_read32(ctx, cam + 0x2C) * 3u | mem_read32(ctx, cam + 0x5C) * 7u | mem_read8(ctx, cam + 0x5B0) << 30;
    for (int i = 0; i < seen_count; i++)
        if (seen[i].cam == cam && seen[i].key == key) return;
    if (seen_count < SEEN_MAX) {
        seen[seen_count].cam = cam;
        seen[seen_count].key = key;
        seen_count++;
    }
    fprintf(stderr,
            "[wide] %s cam=%08X vt=%08X type=%d top=%d k=%.3f fovy=%.4f aspect=%.4f near=%.3f far=%.1f "
            "lrbt=%.1f,%.1f,%.1f,%.1f vp=%d,%d,%d,%d lr=%08X\n",
            who, cam, mem_read32(ctx, cam), type, top_camera(ctx, cam), k, rf(ctx, cam + 0x28), rf(ctx, cam + 0x2C),
            rf(ctx, cam + 0x30), rf(ctx, cam + 0x34), rf(ctx, cam + 0x58), rf(ctx, cam + 0x5C), rf(ctx, cam + 0x60),
            rf(ctx, cam + 0x64), (int16_t)mem_read16(ctx, cam + 0x3F8), (int16_t)mem_read16(ctx, cam + 0x3FA),
            (int16_t)mem_read16(ctx, cam + 0x3FC), (int16_t)mem_read16(ctx, cam + 0x3FE), ctx->r[14]);
}

/* nn::math::MTX44PerspectiveRadC_FAST and the texture projection that
 * follows it, as ICamera::UpdateProj and FieldCamera::UpdateProj write
 * them. pool is the address of the caller's literal pool constants:
 * half, one, zero, minus_one / last-row, the fidx factor, the texture
 * projection's z. Sin and cos are worked out natively rather than from the
 * game's table, which differs in the last bits only. */
static void perspective(Context *ctx, uint32_t cam, float fovy, float aspect, float n, float f,
                        float half, float one, float zero, float w_row, float tex_z) {
    uint32_t P = cam + 0xC8, T = cam + 0x358;
    /* MTX44PerspectiveRadC_FAST's own constants, read from its pool */
    float p_c = rf(ctx, 0x0012EB3C), p_zero = rf(ctx, 0x0012EB40), p_w = rf(ctx, 0x0012EB44);
    float angle = fovy * 0.5f;
    float cot = cosf(angle) / sinf(angle);
    float d = p_c / (f - n);
    for (int i = 0; i < 16; i++) wf(ctx, P + 4 * i, p_zero);
    wf(ctx, P + 0x00, cot / aspect);
    wf(ctx, P + 0x14, cot);
    wf(ctx, P + 0x28, f * d);
    wf(ctx, P + 0x2C, f * n * d);
    wf(ctx, P + 0x38, p_w);
    /* the texture projection */
    float c2 = one / (sinf(angle) / cosf(angle));
    wf(ctx, T + 0x00, c2 / aspect * half);
    wf(ctx, T + 0x04, zero);
    wf(ctx, T + 0x08, tex_z);
    wf(ctx, T + 0x0C, zero);
    wf(ctx, T + 0x10, zero);
    wf(ctx, T + 0x14, c2 * half);
    wf(ctx, T + 0x18, tex_z);
    wf(ctx, T + 0x1C, zero);
    wf(ctx, T + 0x20, zero);
    wf(ctx, T + 0x24, zero);
    wf(ctx, T + 0x28, w_row);
    wf(ctx, T + 0x2C, zero);
}

static void update_proj(Context *ctx, uint32_t cam) {
    /* UpdateProj's literal pool */
    float half = rf(ctx, 0x004DB0EC), one = rf(ctx, 0x004DB0F0), zero = rf(ctx, 0x004DB0F4), m1 = rf(ctx, 0x004DB0F8);
    float two = rf(ctx, 0x004DB0FC), tex_z = rf(ctx, 0x004DB104);
    int type = mem_read8(ctx, cam + 0x41);
    float k = camera_k(ctx, cam);
    log_camera(ctx, cam, type, k, "proj");
    uint32_t P = cam + 0xC8, T = cam + 0x358;
    if (type == 0) {
        perspective(ctx, cam, rf(ctx, cam + 0x28), rf(ctx, cam + 0x2C) * k, rf(ctx, cam + 0x30), rf(ctx, cam + 0x34),
                    half, one, zero, m1, tex_z);
        return;
    }
    float l = rf(ctx, cam + 0x58), r = rf(ctx, cam + 0x5C), b = rf(ctx, cam + 0x60), t = rf(ctx, cam + 0x64);
    float n = rf(ctx, cam + 0x30), f = rf(ctx, cam + 0x34);
    if (k != 1.0f) {
        float c = (l + r) * 0.5f;
        l = c + (l - c) * k;
        r = c + (r - c) * k;
    }
    if (type == 1) {
        /* nn::math::MTX44FrustumC as UpdateProj inlines it */
        float n2 = n * two;
        for (int i = 0; i < 16; i++) wf(ctx, P + 4 * i, zero);
        wf(ctx, P + 0x00, n2 / (r - l));
        wf(ctx, P + 0x08, (r + l) / (r - l));
        wf(ctx, P + 0x14, n2 / (t - b));
        wf(ctx, P + 0x18, (t + b) / (t - b));
        wf(ctx, P + 0x28, f / (f - n));
        wf(ctx, P + 0x2C, f * n / (f - n));
        wf(ctx, P + 0x38, m1);
        wf(ctx, T + 0x00, n2 / (r - l) * half);
        wf(ctx, T + 0x04, zero);
        wf(ctx, T + 0x08, (r + l) / (r - l) * half - half);
        wf(ctx, T + 0x0C, zero);
        wf(ctx, T + 0x10, zero);
        wf(ctx, T + 0x14, n2 / (t - b) * half);
        wf(ctx, T + 0x18, (t + b) / (t - b) * half - half);
        wf(ctx, T + 0x1C, zero);
        wf(ctx, T + 0x20, zero);
        wf(ctx, T + 0x24, zero);
        wf(ctx, T + 0x28, m1);
        wf(ctx, T + 0x2C, zero);
        return;
    }
    if (type == 2) {
        for (int i = 0; i < 16; i++) wf(ctx, P + 4 * i, zero);
        wf(ctx, P + 0x00, two / (r - l));
        wf(ctx, P + 0x0C, -((r + l) / (r - l)));
        wf(ctx, P + 0x14, two / (t - b));
        wf(ctx, P + 0x1C, -((t + b) / (t - b)));
        wf(ctx, P + 0x28, one / (f - n));
        wf(ctx, P + 0x2C, n * (one / (f - n)));
        wf(ctx, P + 0x3C, one);
        wf(ctx, T + 0x00, two / (r - l) * half);
        wf(ctx, T + 0x04, zero);
        wf(ctx, T + 0x08, zero);
        wf(ctx, T + 0x0C, half + (-((r + l) / (r - l))) * half);
        wf(ctx, T + 0x10, zero);
        wf(ctx, T + 0x14, two / (t - b) * half);
        wf(ctx, T + 0x18, zero);
        wf(ctx, T + 0x1C, half + (-((t + b) / (t - b))) * half);
        wf(ctx, T + 0x20, zero);
        wf(ctx, T + 0x24, zero);
        wf(ctx, T + 0x28, zero);
        wf(ctx, T + 0x2C, one);
    }
}

/* ICamera::UpdateProj(void) */
RECOMP_OVERRIDE(0x004DADD4) {
    update_proj(ctx, ctx->r[0]);
    RETURN_TO(ctx->r[14]);
}

/* FieldCamera::UpdateProj(void): the plain ICamera one unless +0x5C8 is
 * set, then a perspective from its own fovy, aspect, near and far at
 * +0x63C..+0x648. */
RECOMP_OVERRIDE(0x001966AC) {
    uint32_t cam = ctx->r[0];
    if (mem_read32(ctx, cam + 0x5C8) == 0) {
        update_proj(ctx, cam);
        RETURN_TO(ctx->r[14]);
    }
    float half = rf(ctx, 0x001967F8), one = rf(ctx, 0x00196800), zero = rf(ctx, 0x00196804);
    float w_row = rf(ctx, 0x00196808), tex_z = rf(ctx, 0x0019680C);
    float k = camera_k(ctx, cam);
    log_camera(ctx, cam, 0x10, k, "field");
    perspective(ctx, cam, rf(ctx, cam + 0x63C), rf(ctx, cam + 0x640) * k, rf(ctx, cam + 0x644), rf(ctx, cam + 0x648),
                half, one, zero, w_row, tex_z);
    RETURN_TO(ctx->r[14]);
}

/* ICamera::GetWorldToScreen(const VEC3&) const: the viewport position of a
 * point, through the camera's view-projection at +0x108 (4x4, before the
 * display rotation), returned in s0, s1, and its depth (z / w) in s2. For a
 * widened camera the x comes out in the widened 2D space, where the
 * original 400 are the middle. */
RECOMP_OVERRIDE(0x00544424) {
    uint32_t cam = ctx->r[0], p = ctx->r[1];
    float x = rf(ctx, p), y = rf(ctx, p + 4), z = rf(ctx, p + 8);
    float m[16];
    for (int i = 0; i < 16; i++) m[i] = rf(ctx, cam + 0x108 + 4 * i);
    float one = rf(ctx, 0x00544520), half = rf(ctx, 0x00544524);
    float cx = m[0] * x + m[1] * y + m[2] * z + m[3];
    float cy = m[4] * x + m[5] * y + m[6] * z + m[7];
    float cz = m[8] * x + m[9] * y + m[10] * z + m[11]; /* gfx-units */
    float cw = m[12] * x + m[13] * y + m[14] * z + m[15];
    float inv = one / cw;
    float sx = (cx * inv + one) * half;
    float sy = (one - cy * inv) * half;
    float vx = (float)(int16_t)mem_read16(ctx, cam + 0x3F8), vy = (float)(int16_t)mem_read16(ctx, cam + 0x3FA);
    float vw = (float)(int16_t)mem_read16(ctx, cam + 0x3FC), vh = (float)(int16_t)mem_read16(ctx, cam + 0x3FE);
    float k = camera_k(ctx, cam);
    /* gfx-units: map::UnitRender::OnDrawXlu (returning to 0x0034BF34)
     * culls its units by this x against the original 400, so it gets the x
     * across the whole widened screen in those 400; gfx_units.c's
     * CalcUnprojected2DMatrixFast widens it back for it */
    if (k != 1.0f && mem_read8(ctx, cam + 0x41) != 2 && ctx->r[14] != 0x0034BF34u) sx = 0.5f + (sx - 0.5f) * k;
    vfp_set_s(ctx, 0, vx + sx * vw);
    vfp_set_s(ctx, 1, vy + sy * vh);
    /* gfx-units: the depth, which the callers that place a sprite in 3D at
     * the point (kaneko::CalcUnprojected2DMatrixFast: map unit sprites and
     * HP gauges, map icons, damage popups...) read. Left out, s2 kept what
     * the caller had in it and the sprites went in front of the near
     * plane, clipped away. */
    vfp_set_s(ctx, 2, cz * inv);
    RETURN_TO(ctx->r[14]);
}

/* ---- the 2D layer's scissors and viewports ----
 * prim::Context::UpdateState (0x00126AF0-0x00127318) sets each state's
 * scissor and viewport in framebuffer coordinates, rotated: x along the
 * screen's 240, y along its 400. On the upper screen the 2D layout keeps
 * its 400 in the middle of the widened screen, so a rectangle that is not
 * the whole width moves and narrows with it: y' = 200 + (y - 200) / k. The
 * display the context is drawing is noted by UpdateWorld, which
 * UpdateState calls first. */
static int prim_display = -1;

static int from_update_state(uint32_t lr) {
    return lr >= 0x00126AF0 && lr < 0x00127318;
}

/* the rows y..y+h of the 400 squeezed to the middle, unless they are all
 * of them. returns 1 when it changed them. */
static int squeeze_rows(float k, int32_t *y, int32_t *h) {
    if (k <= 1.0f || (*y <= 0 && *y + *h >= 400)) return 0;
    float y0 = 200.0f + ((float)*y - 200.0f) / k;
    float y1 = 200.0f + ((float)(*y + *h) - 200.0f) / k;
    int32_t a = (int32_t)floorf(y0 + 0.5f), b = (int32_t)floorf(y1 + 0.5f);
    *y = a;
    *h = b - a;
    return 1;
}

/* prim::Context::UpdateWorld(const ShaderObj*) const */
RECOMP_OVERRIDE(0x00131294) {
    prim_display = mem_read8(ctx, ctx->r[0] + 0x44);
    CALL(RECOMP_ORIGINAL(0x00131294));
}

void gfx_post_observe(Context *ctx, uint32_t addr); /* gfx-post: gfx_post.c */

/* Pica::ScissorTest(bool enable, int x, int y, int w, int h), h on the
 * stack */
RECOMP_OVERRIDE(0x00125F60) {
    gfx_post_observe(ctx, 0x00125F60); /* gfx-post: native post-effect layer */
    if (ctx->r[0] && prim_display == 0 && from_update_state(ctx->r[14])) {
        int32_t y = (int32_t)ctx->r[2], h = (int32_t)mem_read32(ctx, ctx->r[13]);
        if (squeeze_rows(wide_k(), &y, &h)) {
            ctx->r[2] = (uint32_t)y;
            mem_write32(ctx, ctx->r[13], (uint32_t)h);
        }
    }
    CALL(RECOMP_ORIGINAL(0x00125F60));
}

/* Pica::Viewport(unsigned x, unsigned y, unsigned w, unsigned h) */
RECOMP_OVERRIDE(0x001263E0) {
    gfx_post_observe(ctx, 0x001263E0); /* gfx-post: native post-effect layer */
    if (prim_display == 0 && from_update_state(ctx->r[14])) {
        int32_t y = (int32_t)ctx->r[1], h = (int32_t)ctx->r[3];
        if (squeeze_rows(wide_k(), &y, &h)) {
            ctx->r[1] = (uint32_t)(y < 0 ? 0 : y);
            ctx->r[3] = (uint32_t)h;
        }
    }
    CALL(RECOMP_ORIGINAL(0x001263E0));
}

/* the camera the prim state being recorded draws with: the one its word 0
 * names (bits 13-16), or the target's own (bits 3-4), as
 * prim::Context::UpdateState picks it. 0 for none. */
static uint32_t prim_camera(Context *ctx, uint32_t prim) {
    uint32_t state = mem_read32(ctx, prim + 0x18);
    if (!state) return 0;
    uint32_t w0 = mem_read32(ctx, state);
    uint32_t index = (w0 & 0x1E000) ? (w0 & 0x1FFFF) >> 13 : (w0 & 0x1F) >> 3;
    uint32_t first = mem_read32(ctx, prim + 0x2458), end = mem_read32(ctx, prim + 0x245C);
    if (end <= first || index >= (end - first) / 4) return 0;
    return mem_read32(ctx, first + index * 4);
}

/* prim::Context::DrawRect(x, y, z, w, h, u0, v0, u1, v1, const Color8&):
 * s0..s4 x, y, z, w, h. A rectangle across the whole width of the top
 * screen's 2D camera, a fade or a full-screen background, is stretched
 * across the widened screen; everything else keeps its place and size,
 * centred. Only the arguments change, then the game's own function runs
 * (as a tail call, so nothing is lost if it yields). */
RECOMP_OVERRIDE(0x00416410) {
    float k = wide_k();
    if (k > 1.0f) {
        uint32_t cam = prim_camera(ctx, ctx->r[0]);
        if (cam && top_camera(ctx, cam) && mem_read8(ctx, cam + 0x41) == 2) {
            float l = rf(ctx, cam + 0x58), r = rf(ctx, cam + 0x5C);
            float x = vfp_s(ctx, 0), w = vfp_s(ctx, 3);
            float lo = w < 0 ? x + w : x, hi = w < 0 ? x : x + w;
            float span = r > l ? r - l : l - r, left = r > l ? l : r, right = r > l ? r : l;
            if (getenv("FATES_WIDE_RECTS")) {
                static int n;
                if (n++ < 4000)
                    fprintf(stderr, "[wide] rect x=%.1f y=%.1f w=%.1f h=%.1f uv=%.3f,%.3f-%.3f,%.3f lr=%08X\n", x, vfp_s(ctx, 1), w,
                            vfp_s(ctx, 4), vfp_s(ctx, 5), vfp_s(ctx, 6), vfp_s(ctx, 7), vfp_s(ctx, 8), ctx->r[14]);
            }
            /* movies (mw::mo::helper::GlVideoPresentation::Draw,
             * 0x00225D30-0x00225DF8) keep their shape, pillarboxed */
            uint32_t from = ctx->r[14];
            int movie = from >= 0x00225D30 && from < 0x00225DF8;
            if (!movie && span > 0.0f && lo <= left + 1.0f && hi >= right - 1.0f) {
                float c = (l + r) * 0.5f;
                vfp_set_s(ctx, 0, c + (x - c) * k);
                vfp_set_s(ctx, 3, w * k);
                /* wide2: a textured one (a wallpaper) is told to the native
                 * renderer, which draws it at its own size and extends its
                 * edges instead; the emulated GPU keeps it stretched */
                if (fates_wide2_rect_hook) {
                    uint32_t prim = ctx->r[0], state = mem_read32(ctx, prim + 0x18);
                    uint32_t table = mem_read32(ctx, prim + 0x2464);
                    if (state && table && mem_read32(ctx, table + 4u * mem_read8(ctx, state + 0xC)))
                        fates_wide2_rect_hook(state, mem_read16(ctx, state + 0x12) / 4u, c, k);
                }
                if (wide_log()) {
                    static int logged;
                    if (logged++ < 40)
                        fprintf(stderr, "[wide] full-width rect x=%.1f w=%.1f cam=%08X lr=%08X\n", x, w, cam, ctx->r[14]);
                }
            }
        }
    }
    CALL(RECOMP_ORIGINAL(0x00416410));
}

/* ---- wide3: the battle map's camera kept over the map ----
 * game::graphics::CameraBase::GetLimitTarget(const VEC3 &target, float
 * distance) (0x00536710) clamps where the map camera looks, in tiles: x
 * between the play area's left + m and right - m, m a margin of
 * FieldWorld's parameters (+0x8C at the nearest distance +0x80, +0x98 at
 * the farthest +0x88, interpolated by the distance; a play area narrower
 * than 2m centres). The margins were set for the 3DS's 5:3, whose half
 * view at the target is about H = distance * tan(fovy / 2) * aspect tiles;
 * the map's mesh ends a few tiles past the play area, as much as the 3DS
 * view ever shows. A view k times wider shows (k - 1) H more on each side,
 * so on a map the camera can reach the edge of, a wide view showed the void
 * past the mesh (Ghostly Gold at 32:9). Here x is clamped again with each
 * margin grown by (k - 1) H F (F = FATES_WIDE3_MAPF, default 1): at the
 * limit the wide view's edges at the target's depth stay where the 3DS
 * view's were, and everything the 3DS view showed is still in view. The
 * rows at the top of the screen see a little farther out, (k - 1) times
 * their extra width over the centre's, which is less than what 21:9 showed
 * unclamped, where the mesh was always enough (Ghostly Gold at 32:9: no
 * void with F = 1; 1.35 moved the camera 2.6 tiles more for nothing). A map narrower than the
 * wider view centres, as the game does when its own margins meet. Only the
 * map's camera (map::Camera), not My Castle's. FATES_WIDE3_MAPLIMIT=0
 * turns it off. The tail runs after the original (CALL_THEN, no guest
 * calls); it reads the same parameters the original read. */
static float wide3_map_f(void) {
    static float f = -1.0f;
    if (f < 0.0f) {
        const char *v = getenv("FATES_WIDE3_MAPF");
        f = v ? (float)atof(v) : 1.0f;
        if (getenv("FATES_WIDE3_MAPLIMIT") && atoi(getenv("FATES_WIDE3_MAPLIMIT")) == 0) f = 0.0f;
    }
    return f;
}

static float wide3_clamp(float x, float lo, float hi, float half) {
    if (lo > hi) return (lo + hi) * half;
    if (x >= hi) return hi;
    if (x > lo) return x;
    return lo;
}

/* words: the camera, the target, the distance's bits */
static void wide3_limit_end(Context *ctx, const uint32_t *w) {
    uint32_t cam = w[0], target = w[1];
    float k = wide_k(), f = wide3_map_f();
    if (k <= 1.0f || f <= 0.0f) return;
    /* map::Camera's vtable, as map::Camera::Initialize stores it */
    if (mem_read32(ctx, cam) != mem_read32(ctx, 0x0039B6D0)) return;
    /* FieldWorld::GetParam: *(*(*0x006BE404 + 8) + 0x94) */
    uint32_t world = mem_read32(ctx, mem_read32(ctx, 0x00178294));
    if (!world) return;
    uint32_t param = mem_read32(ctx, mem_read32(ctx, world + 8) + 0x94);
    /* the map's play area: map::Camera::GetPlayAreaX / W */
    uint32_t map = mem_read32(ctx, mem_read32(ctx, 0x005270F8));
    if (!param || !map) return;
    float x0 = (float)mem_read8(ctx, map + 2);
    float x1 = x0 + (float)(uint8_t)(mem_read8(ctx, map + 4) - mem_read8(ctx, map + 2));
    float dist = vfp_from_s(w[2]);
    float near_d = rf(ctx, param + 0x80), far_d = rf(ctx, param + 0x88);
    float m_near = rf(ctx, param + 0x8C), m_far = rf(ctx, param + 0x98);
    float half = rf(ctx, 0x00536A8C), scale = rf(ctx, 0x00536A90);
    if (!(far_d != near_d)) return;
    float t = (dist - near_d) / (far_d - near_d);
    float fovy = rf(ctx, cam + 0x28), aspect = rf(ctx, cam + 0x2C);
    if (!(fovy > 0.0f && fovy < 3.0f && aspect > 0.0f)) return;
    float grow = (k - 1.0f) * dist * tanf(fovy * 0.5f) * aspect * f;
    float x = rf(ctx, target);
    float xn = wide3_clamp(x, x0 + m_near + grow, x1 - m_near - grow, half);
    float xf = wide3_clamp(x, x0 + m_far + grow, x1 - m_far - grow, half);
    float out = xn + (xf - xn) * t * scale;
    if (wide_log()) {
        static int logged;
        if (logged++ < 40)
            fprintf(stderr, "[wide3] map limit x %.2f -> %.2f (game %.2f) area %.0f..%.0f dist %.1f grow %.2f\n", x, out,
                    vfp_s(ctx, 0), x0, x1, dist, grow);
    }
    vfp_set_s(ctx, 0, out);
}

/* game::graphics::CameraBase::GetLimitTarget(const VEC3&, float) const */
RECOMP_OVERRIDE(0x00536710) {
    /* s0: the distance */
    CALL_THEN(RECOMP_ORIGINAL(0x00536710), wide3_limit_end, ctx->r[0], ctx->r[1], ctx->vfp[0]);
}

/* ---- applying a new aspect to cameras that are set up once ----
 * The 2D layer's orthographic cameras run UpdateProj only when they are set
 * up, so a camera made at one aspect (or brought back by a saved state)
 * keeps its projection when the aspect changes. ICamera::GetViewProj, which
 * every 2D state asks for its matrix, puts such a camera right first: the
 * widening it has is read back from its own projection (P[0] against its
 * left and right), and the rows of its matrices that hold the horizontal
 * are scaled by what is missing. Perspective cameras run UpdateProj every
 * frame and need nothing here. */

/* the row, 0 or 1, of a pivoted 4x4 that holds what was row 0 before the
 * display rotation: the one with the larger x coefficient. */
static int horizontal_row(Context *ctx, uint32_t m) {
    float a = fabsf(rf(ctx, m + 0x00)), b = fabsf(rf(ctx, m + 0x10));
    return a >= b ? 0 : 1;
}

static void scale_row(Context *ctx, uint32_t m, int row, float s) {
    for (int i = 0; i < 4; i++) {
        uint32_t at = m + row * 0x10 + i * 4;
        wf(ctx, at, rf(ctx, at) * s);
    }
}

static void refresh_ortho(Context *ctx, uint32_t cam) {
    if (mem_read8(ctx, cam + 0x41) != 2 || mem_read8(ctx, cam + 0x5B0) == 0) return;
    float l = rf(ctx, cam + 0x58), r = rf(ctx, cam + 0x5C), p0 = rf(ctx, cam + 0xC8);
    float two = rf(ctx, 0x004DB0FC);
    if (r == l || p0 == 0.0f) return;
    float applied = fabsf(two / ((r - l) * p0));
    float wanted = camera_k(ctx, cam);
    if (!(applied > 0.5f) || fabsf(applied - wanted) < 0.001f * wanted) return;
    float s = applied / wanted;
    int row = horizontal_row(ctx, cam + 0x1A8);
    /* P before the rotation, its row 0 */
    wf(ctx, cam + 0xC8, p0 * s);
    wf(ctx, cam + 0xD4, rf(ctx, cam + 0xD4) * s);
    scale_row(ctx, cam + 0x108, 0, s);
    scale_row(ctx, cam + 0x1A8, row, s);
    scale_row(ctx, cam + 0x1E8, row, s);
    scale_row(ctx, cam + 0x228, row, s);
    scale_row(ctx, cam + 0x268, row, s);
    if (wide_log()) fprintf(stderr, "[wide] refreshed ortho cam=%08X from k=%.3f to k=%.3f (row %d)\n", cam, applied, wanted, row);
}

/* ICamera::GetViewProj(int eye) const: cam + 0x228, or + 0x268 for the
 * right eye. */
RECOMP_OVERRIDE(0x001315FC) {
    uint32_t cam = ctx->r[0];
    refresh_ortho(ctx, cam);
    ctx->r[0] = cam + (ctx->r[1] ? 0x268 : 0x228);
    RETURN_TO(ctx->r[14]);
}

/* prim::Context::EndState(void): registered for the 2D layer's full-width
 * states (see the report); passes through for now. */
RECOMP_OVERRIDE(0x004164D0) {
    CALL(RECOMP_ORIGINAL(0x004164D0));
}
