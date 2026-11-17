/* gfx-units: 2D sprites placed in 3D (map unit sprites, their HP gauges,
 * map icons, damage popups) on a widened top screen.
 *
 * Those callers take a point's screen position from
 * ICamera::GetWorldToScreen (0x00544424) and hand it, with its depth, to
 * kaneko::CalcUnprojected2DMatrixFast (0x0049DB2C), which unprojects the
 * pixel at that position, the pixel right of it and the one below it
 * through the camera's inverse view-projection (camera + 0x318) into the
 * sprite's world matrix: a sprite one 2D unit per pixel, at the point's
 * depth, facing the camera.
 *
 * On a widened top screen (overrides/widescreen.c) GetWorldToScreen gives
 * x in the widened 2D space, where the original 400 are the middle, while
 * camera + 0x318 is the inverse of the widened projection, whose x runs
 * -1..1 across the whole widened screen. Unprojected as the game does it,
 * a sprite lands k times too far from the middle and comes out k times too
 * wide. For a widened camera this override does the same work with the
 * widened x brought back into the projection's range and the pixel step
 * divided by k, so the sprite sits on its point and keeps the 2D layer's
 * pixel size. Any other camera, or the console's aspect, runs the original.
 *
 * map::UnitRender::OnDrawXlu (calling from 0x0034BF7C) culls its units by
 * GetWorldToScreen's x against the original 400 first, so widescreen.c
 * gives it the x across the whole widened screen in those 400 instead; it
 * is widened back here.
 *
 * Addresses: USA SE v2. */
#include <math.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include "overrides.h"

extern float (*fates_wide_aspect_hook)(void); /* widescreen.c */

/* gfx-2d: how finely a sprite's screen position is snapped, in parts of a
 * 2D pixel: the host's render scale (fates_gfx::units_snap). The game
 * truncates to whole console pixels, which keeps its pixel art on the
 * console's pixel grid; drawn 8 times larger, units then moved in 8-pixel
 * steps while the map under them glided with the camera, and looked
 * jittery whenever the cursor scrolled the map. Snapped to the render's
 * own pixels instead, the art still lands on whole render pixels (crisp
 * with nearest sampling) and moves with the terrain. Unset or 1: the
 * game's own whole pixels, the original function. */
int (*fates_units_snap_hook)(void) = 0;

static float units_snap(void) {
    int s = fates_units_snap_hook ? fates_units_snap_hook() : 1;
    return s > 1 ? (float)s : 1.0f;
}

/* widescreen.c's wide_k: the top screen's widening (1 for none) */
static float units_wide_k(void) {
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
    if (!(aspect > 400.0f / 240.0f + 0.001f)) return 1.0f;
    return aspect / (400.0f / 240.0f);
}

static float units_rf(Context *ctx, uint32_t a) { return vfp_from_s(mem_read32(ctx, a)); }

static void units_wf(Context *ctx, uint32_t a, float v) {
    uint32_t bits;
    memcpy(&bits, &v, 4);
    mem_write32(ctx, a, bits);
}

/* vcvt.s32.f32: toward zero, saturating */
static int32_t units_trunc(float v) {
    if (!(v == v)) return 0;
    if (v >= 2147483647.0f) return 2147483647;
    if (v <= -2147483648.0f) return (int32_t)0x80000000u;
    return (int32_t)v;
}

/* the widened top-screen perspective camera whose inverse view-projection
 * (+0x318) is `m`, or 0. Its last row is the inverse projection's (+0x2D8),
 * as ICamera::UpdateWorld makes it; with the widescreen.c test for a camera
 * drawing the whole top screen. */
static uint32_t units_wide_camera(Context *ctx, uint32_t m) {
    if (m < 0x318) return 0;
    uint32_t cam = m - 0x318;
    uint32_t any = 0;
    for (int i = 12; i < 16; i++) {
        uint32_t a = mem_read32(ctx, m + 4 * i);
        if (a != mem_read32(ctx, cam + 0x2D8 + 4 * i)) return 0;
        any |= a;
    }
    if (!any || mem_read8(ctx, cam + 0x41) != 0 || mem_read8(ctx, cam + 0x5B0) == 0) return 0;
    int16_t vx = (int16_t)mem_read16(ctx, cam + 0x3F8), vw = (int16_t)mem_read16(ctx, cam + 0x3FC);
    if (!(vx <= 0 && vx + vw >= 400)) return 0;
    return cam;
}

/* kaneko::CalcUnprojected2DMatrixFast(nn::math::MTX34* out,
 * const nn::math::VEC3& screen, const nn::math::MTX44& unprojection) */
RECOMP_OVERRIDE(0x0049DB2C) {
    float k = units_wide_k();
    uint32_t out = ctx->r[0], p = ctx->r[1], m = ctx->r[2];
    uint32_t cam = k > 1.0f ? units_wide_camera(ctx, m) : 0;
    float snap = units_snap(); /* gfx-2d */
    if (!cam && snap <= 1.0f) {
        CALL(RECOMP_ORIGINAL(0x0049DB2C));
        return;
    }
    if (!cam) k = 1.0f; /* gfx-2d: the original's work, finer snapped */
    /* its literal pool: the x and y size of a 2D unit in the projection's
     * -1..1, one, zero */
    float cx = units_rf(ctx, 0x0049DD08), cy = units_rf(ctx, 0x0049DD0C);
    float one = units_rf(ctx, 0x0049DD10), zero = units_rf(ctx, 0x0049DD14);
    float sx = units_rf(ctx, p), sy = units_rf(ctx, p + 4);
    uint32_t zbits = mem_read32(ctx, p + 8);
    float z = vfp_from_s(zbits);
    if (cam && ctx->r[14] == 0x0034BF80u) {
        /* map::UnitRender::OnDrawXlu: x across the widened screen in 400,
         * back into the widened 2D space (see widescreen.c) */
        float vx = (float)(int16_t)mem_read16(ctx, cam + 0x3F8), vw = (float)(int16_t)mem_read16(ctx, cam + 0x3FC);
        float c = vx + vw * 0.5f;
        sx = c + (sx - c) * k;
    }
    /* gfx-2d: truncated to 1/snap of a pixel (snap 1: the game's whole
     * pixels) */
    float x0 = ((float)units_trunc(sx * snap) / snap * cx - one) / k;
    float y0 = one + (float)(-units_trunc(sy * snap)) / snap * cy;
    float pt[3][3] = {{x0, y0, z}, {x0 + cx / k, y0, z}, {x0, y0 - cy, z}};
    float M[16];
    for (int i = 0; i < 16; i++) M[i] = units_rf(ctx, m + 4 * i);
    float r[3][3];
    for (int i = 0; i < 3; i++) {
        float x = pt[i][0], y = pt[i][1], zz = pt[i][2];
        float w = (M[13] * y + M[14] * zz) + (M[15] + M[12] * x);
        float inv = one / w;
        r[i][0] = ((M[1] * y + M[2] * zz) + (M[3] + M[0] * x)) * inv;
        r[i][1] = ((M[5] * y + M[6] * zz) + (M[7] + M[4] * x)) * inv;
        r[i][2] = ((M[9] * y + M[10] * zz) + (M[11] + M[8] * x)) * inv;
    }
    units_wf(ctx, out + 0x00, r[1][0] - r[0][0]);
    units_wf(ctx, out + 0x04, r[2][0] - r[0][0]);
    units_wf(ctx, out + 0x08, zero);
    units_wf(ctx, out + 0x0C, r[0][0]);
    units_wf(ctx, out + 0x10, r[1][1] - r[0][1]);
    units_wf(ctx, out + 0x14, r[2][1] - r[0][1]);
    units_wf(ctx, out + 0x18, zero);
    units_wf(ctx, out + 0x1C, r[0][1]);
    units_wf(ctx, out + 0x20, r[1][2] - r[0][2]);
    units_wf(ctx, out + 0x24, r[2][2] - r[0][2]);
    units_wf(ctx, out + 0x28, one);
    units_wf(ctx, out + 0x2C, r[0][2]);
    RETURN_TO(ctx->r[14]);
}
