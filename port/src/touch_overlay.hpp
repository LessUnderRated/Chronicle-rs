#pragma once

#include <cstdint>

#include "gfx/gfx.hpp"

// The touch controls drawn over every presented frame on Android (android/src/touch.rs); nothing
// elsewhere.

// Brings the controls up to date for the main target's UI mapping. Their picture's identity: it
// changes whenever the picture does, and is 0 while there is none.
std::uint64_t TouchOverlayUpdate(const gfx::LogicalMapping &mapping);

// Draws the picture TouchOverlayUpdate last made, inside a frame or a recording.
void TouchOverlayDraw(const gfx::LogicalMapping &mapping);
