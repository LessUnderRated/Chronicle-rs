#pragma once

#include <filesystem>
#include <string>

// A save state: the whole running game in one file. In the layout-table mode (the build ships
// states_layout.bin, scripts/port/game_ranges.py) a state holds only the game's own globals and the
// arena blocks, both addressed relative to things that stay put or move in known ways:
//
//   - the arena blocks live in Kazarov's fixed reservation, whose address the same candidates pick
//     on every launch, so the game's pointers into them — whole and truncated — keep their meaning;
//   - the game's globals are recorded at offsets from the code image's base, and a load rewrites
//     the 8-byte values that named the old image by the base difference (a vtable pointer, a static
//     string, a function pointer);
//   - the host's own globals — the renderer's, the texture registry's, this module's — are not in
//     the state at all: they belong to the running session, whose native heap the restored game
//     cannot know. What they cache of the game (textures, banks) the game rebuilds as it runs.
//
// The 32-bit image pointers the game truncates (a character's cloth wind) are not rebased: their
// reader widens against the running image and takes a null for no wind, which a stale value
// becomes. A state without a table holds the whole image instead and is only good for the session
// that saved it.
//
// Saving and loading take place between ticks on the game's thread, with the audio output stopped
// (the caller's job), so nothing reads the memory as it is written over.

bool GameStateSave(const std::filesystem::path &path, std::string *error);
bool GameStateLoad(const std::filesystem::path &path, std::string *error);

// A quick save or load the app's menu asked for, taken between ticks: `save` or `load` into the
// save root's states/ folder, numbered by slot. The result is written beside the state as
// last.json for the menu to show, and to stderr for the log.
void GameStateRequest(bool save, int slot);
// Runs one pending request, if there is one; the caller stops the audio output around it.
bool GameStateTakeRequest(bool *save, int *slot);
bool GameStateHasPending();
// Runs one request at once: the save root's states/slot-<N>.dcst, and the result beside it.
bool GameStateRunRequest(const std::filesystem::path &save_root, bool save, int slot);
// Applies every queued menu request. True if a load ran. Call after initialize_data and
// SndInit, or at a tick of RunGame, never from a clock pump inside DataLoad.
bool GameStateApplyPending();