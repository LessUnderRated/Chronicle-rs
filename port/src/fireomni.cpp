#include "fireomni.hpp"

#include <libgraph.h>

#include <cstdlib>

#include "camera.hpp"
#include "draw2d_port.hpp"
#include "mglib.hpp"
#include "rect.hpp"
#include "runeffect.hpp"
#include "snd.hpp"
#include "texture.hpp"

namespace {

// Retail's raw TEX1/TEST/ZBUF/ALPHA writes ahead of each layer: no alpha test, depth tested but not
// written, additive Cs * As + Cd. The sprites pick TEX1 themselves.
void SetLayerRegisters() {
    const draw2d::Services &services = draw2d::Get();
    sceGsTest               test = mgPixelTest;
    test.bits.ate = 0;
    test.bits.zte = 1;
    services.set_test(&test);
    sceGsZbuf zbuffer = mgZBuffer;
    zbuffer.bits.zmsk = 1;
    services.set_zbuf(&zbuffer);
    sceGsAlpha alpha = mgAlpha;
    alpha.bits.a = 0;
    alpha.bits.b = 2;
    alpha.bits.c = 0;
    alpha.bits.d = 1;
    services.set_alpha(&alpha);
}

} // namespace

PORT_OVERRIDE void CFireOmni::DrawFire(int unused0, int unused1, CCamera *camera, float *colour, float scale, int layers,
                                     float camera_offset) {
    sceVu0FVECTOR camera_direction;
    sceVu0FVECTOR camera_ref;
    int           near_top_left[4];
    int           near_bottom_right[4];
    sceVu0FVECTOR world;
    int           top_left[4];
    int           bottom_right[4];
    int           top_right[4];
    int           bottom_left[4];

    if (texture_set == 0) {
        if (core == NULL) {
            core = TexManager.GetTexture(const_cast<char *>("lightling"), -1);
        }

        if (glow == NULL) {
            glow = TexManager.GetTexture(const_cast<char *>("blender"), -1);
        }
    }

    camera->GetPos(camera_direction);
    camera->GetRef(camera_ref);
    sceVu0SubVector(camera_direction, camera_direction, camera_ref);
    sceVu0Normalize(camera_direction, camera_direction);
    sceVu0ScaleVector(camera_direction, camera_direction, camera_offset);
    spRGBA white = {0x80, 0x80, 0x80, 0x80};

    if (layers & 1) {
        SetLayerRegisters();

        world[0] = pos[0];
        world[1] = pos[1] + 4.6f;
        world[2] = pos[2];
        world[3] = 1.0f;

        if (MGRotTransPers3DSprite(top_left, bottom_right, world, 18.0f * scale, 9.0f * scale, 1) == 1) {
            set3DSpriteFog(Vif1Packet, core, CRect_i_(0, 0, 64, 64), top_left, bottom_right, &white);
            set3DSpriteFog(Vif1Packet, glow, CRect_i_(2, 2, 124, 124), top_left, bottom_right, &white);
        }
    }

    if (layers & 2) {
        SetLayerRegisters();

        flicker_count++;
        srand(flicker_seed);

        if (flicker_count >= 5) {
            flicker_height = flicker_width = 1.0f + 0.1f * rand() / 2147483648.0f;
            flicker_count = 0;
        }

        world[0] = pos[0];
        world[1] = pos[1] + 4.6f;
        world[2] = pos[2];
        world[3] = 1.0f;

        if (MGRotTransPers3DSprite(top_left, bottom_right, world, 45.0f * flicker_width * scale,
                                   45.0f * flicker_height * scale / 2.0f, 1) == 1) {
            top_right[0] = bottom_right[0];
            top_right[1] = top_left[1];
            top_right[2] = top_left[2];
            top_right[3] = top_left[3];
            bottom_left[0] = top_left[0];
            bottom_left[1] = bottom_right[1];
            bottom_left[2] = bottom_right[2];
            bottom_left[3] = bottom_right[3];
            world[0] += camera_direction[0];
            world[1] += camera_direction[1];
            world[2] += camera_direction[2];

            if (MGRotTransPers3DSprite(near_top_left, near_bottom_right, world, 45.0f * scale + flicker_width,
                                       (45.0f * scale + flicker_height) / 2.0f, 0) == 1) {
                top_right[2] = top_left[2] = bottom_left[2] = bottom_right[2] = near_top_left[2];
            }

            set3DSpriteFog(Vif1Packet, core, CRect_i_(0, 0, 64, 64), top_left, top_right, bottom_left, bottom_right,
                           0x80);
        }
    }

    const draw2d::Services &services = draw2d::Get();
    services.set_test(nullptr);
    services.set_zbuf(nullptr);
    services.set_alpha(nullptr);
}

// Retail clips the heat raster's rectangle to the frame's 640 columns and copies it through
// "blender", a 640-wide target. A torch wholly past the frame's sides was off screen at 4:3, but a
// wider window shows it, and its raster would copy from outside blender; such a raster is left out
// (the fire itself still draws). One across the frame's edge is clipped as retail clips it.
PORT_OVERRIDE void CFireOmni::DrawRaster() {
    sceVu0FVECTOR world;
    int           top_left[4];
    int           bottom_right[4];

    world[0] = pos[0];
    world[1] = pos[1] + 3.0f;
    world[2] = pos[2];
    world[3] = 1.0f;

    if (MGRotTransPers3DSprite(top_left, bottom_right, world, 15.0f, 16.0f, 0) != 1) {
        return;
    }

    int x = (top_left[0] - 0x6C00) >> 4;
    int y = (top_left[1] - GS_Y_OFFSET) >> 3;
    int width = (bottom_right[0] - top_left[0]) >> 4;
    int height = (bottom_right[1] - top_left[1]) >> 4;
    height >>= 1;

    if (x >= 640 || x + width <= 0) {
        return;
    }

    if (x + width > 639 && x < 640) {
        width = 639 - x;
    }

    if (x < 0 && x + width > 0) {
        width = x + width;
        x = 0;
    }

    if (y < 1) {
        height += y >> 1;
        y = 0;
    }

    sceGsTex0 blend = *reinterpret_cast<sceGsTex0 *>(&TexManager.GetTexture(const_cast<char *>("blender"), -1)->tex0);
    blendTextuerTest(Vif1Packet, blend.TBP0, blend.TBW, blend.PSM, CRect_i_(x, y >> 1, width, height),
                     TexManager.GetTexture(const_cast<char *>("alpha01"), -1), CRect_i_(0, 0, width, height), CRect_i_(0, 0, 64, 64),
                     top_left[2] >> 4, raster_phase);
    set2DSprite(Vif1Packet, TexManager.GetTexture(const_cast<char *>("blender"), -1), CRect_i_(x, y, width, height * 2),
                CRect_i_(1, 1, width - 2, height - 2));
}
