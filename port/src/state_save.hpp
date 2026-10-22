#pragma once

#include <filesystem>
#include <string>

// A save state: the whole running game in one file — its arena blocks and the executable image's
// writable globals (.data and .bss), which between them hold everything the game's logic refers
// to. Everything else the process holds is either rebuilt every frame (the display lists, the
// textures the game re-uploads from its own memory, the frame grabs) or is the host's own plumbing
// (Vulkan handles, the mixer's output), which the restore leaves alone and the next frames refill.
//
// A state is per session: it records raw addresses, and a fresh process lays its image and its
// arenas elsewhere. Saving and loading take place between ticks on the game's thread, with the
// audio output stopped (the caller's job), so nothing reads the memory as it is written over.

bool GameStateSave(const std::filesystem::path &path, std::string *error);
bool GameStateLoad(const std::filesystem::path &path, std::string *error);