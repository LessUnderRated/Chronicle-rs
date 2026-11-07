#include <gtest/gtest.h>

#include <cstdint>
#include <cstring>
#include <filesystem>
#include <string>

#include "platform/arena.hpp"
#include "state_save.hpp"

namespace fs = std::filesystem;

namespace {

fs::path ScratchState() {
    static unsigned sequence = 0;
    return fs::temp_directory_path() / ("dc_state_test_" + std::to_string(sequence++) + ".dcst");
}

} // namespace

// The two kinds of memory a state carries: a global in the image's writable data and a block of
// the game's arenas. A save, some damage, and a load put both back.
TEST(StateSave, RoundTripRestoresGlobalsAndArenas) {
    static std::uint64_t global = 0x1122334455667788;
    static char          key;
    unsigned char       *block = ArenaBlock(&key, 4096);
    for (int i = 0; i < 4096; i++) {
        block[i] = static_cast<unsigned char>(i);
    }
    std::uint64_t before = global;

    fs::path path = ScratchState();
    std::string error;
    ASSERT_TRUE(GameStateSave(path, &error)) << error;

    global ^= 0xFFFFFFFFFFFFFFFF;
    for (int i = 0; i < 4096; i++) {
        block[i] = 0;
    }

    ASSERT_TRUE(GameStateLoad(path, &error)) << error;
    EXPECT_EQ(global, before);
    for (int i = 0; i < 4096; i++) {
        ASSERT_EQ(block[i], static_cast<unsigned char>(i)) << "byte " << i;
    }
    std::error_code ignored;
    fs::remove(path, ignored);
}

// A block the owner grew after the save goes back to the recorded shape, at the recorded address,
// since the state's pointers reach for it there.
TEST(StateSave, LoadPutsAGrownBlockBackWhereItWas) {
    static char key;
    unsigned char *small = ArenaBlock(&key, 1024);
    unsigned char *recorded = small;
    small[0] = 7;

    fs::path path = ScratchState();
    std::string error;
    ASSERT_TRUE(GameStateSave(path, &error)) << error;

    unsigned char *grown = ArenaBlock(&key, 1 << 20);
    ASSERT_NE(grown, recorded);
    grown[0] = 9;

    ASSERT_TRUE(GameStateLoad(path, &error)) << error;
    unsigned char *back = ArenaBlock(&key, 1024);
    EXPECT_EQ(back, recorded);
    EXPECT_EQ(back[0], 7);
    std::error_code ignored;
    fs::remove(path, ignored);
}

// A block carved after the save holds only state the restored game cannot reference, so it reads
// as zeroes, the way a retired block does.
TEST(StateSave, LoadZeroesBlocksTheStateDoesNotHold) {
    static char kept_key;
    static char extra_key;
    unsigned char *kept = ArenaBlock(&kept_key, 1024);
    for (int i = 0; i < 1024; i++) {
        kept[i] = 0xAA;
    }

    fs::path path = ScratchState();
    std::string error;
    ASSERT_TRUE(GameStateSave(path, &error)) << error;

    unsigned char *extra = ArenaBlock(&extra_key, 1024);
    for (int i = 0; i < 1024; i++) {
        extra[i] = 0xBB;
    }

    ASSERT_TRUE(GameStateLoad(path, &error)) << error;
    for (int i = 0; i < 1024; i++) {
        ASSERT_EQ(kept[i], 0xAA);
    }
    for (int i = 0; i < 1024; i++) {
        ASSERT_EQ(extra[i], 0) << "byte " << i;
    }
    std::error_code ignored;
    fs::remove(path, ignored);
}

// A file that is not a state is refused, and says why.
TEST(StateSave, RefusesForeignFiles) {
    fs::path path = ScratchState();
    std::FILE *file = std::fopen(path.string().c_str(), "wb");
    ASSERT_NE(file, nullptr);
    std::fputs("not a state", file);
    std::fclose(file);

    std::string error;
    EXPECT_FALSE(GameStateLoad(path, &error));
    EXPECT_FALSE(error.empty());
    std::error_code ignored;
    fs::remove(path, ignored);
}