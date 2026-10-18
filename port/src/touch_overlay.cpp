#include "touch_overlay.hpp"

#ifdef __ANDROID__

#include <cstddef>
#include <vector>

namespace {

struct TouchVertex {
    float        x;
    float        y;
    std::uint8_t color[4];
};

std::vector<TouchVertex> g_vertices(32768);
std::size_t              g_count = 0;

} // namespace

extern "C" std::uint64_t chronicle_touch_generation();
extern "C" std::size_t   chronicle_touch_geometry(std::uint32_t width, std::uint32_t height, TouchVertex *out,
                                                  std::size_t capacity);

std::uint64_t TouchOverlayUpdate(const gfx::LogicalMapping &mapping) {
    g_count = chronicle_touch_geometry(mapping.pixel_width, mapping.pixel_height, g_vertices.data(), g_vertices.size());
    return g_count == 0 ? 0 : chronicle_touch_generation();
}

void TouchOverlayDraw(const gfx::LogicalMapping &mapping) {
    if (g_count == 0 || mapping.scale_x <= 0.0f || mapping.scale_y <= 0.0f) {
        return;
    }
    std::vector<gfx::Vertex2D> vertices;
    vertices.reserve(g_count);
    for (std::size_t i = 0; i < g_count; i++) {
        const TouchVertex &v = g_vertices[i];
        vertices.push_back(gfx::Vertex2D{
            (v.x - mapping.offset_x) / mapping.scale_x, (v.y - mapping.offset_y) / mapping.scale_y, 0.0f, 0.0f, 0.0f, {v.color[0], v.color[1], v.color[2], v.color[3]}
        });
    }
    gfx::DrawState blended;
    blended.blend = true;
    gfx::Draw2D(gfx::Primitive::Triangles, vertices, {}, blended);
}

#else

std::uint64_t TouchOverlayUpdate(const gfx::LogicalMapping &) {
    return 0;
}

void TouchOverlayDraw(const gfx::LogicalMapping &) {}

#endif
