#include "state_save.hpp"

#include <SDL3/SDL.h>

#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <vector>

#include "platform/arena.hpp"
#include "platform/memory.hpp"
#include "draw3d.hpp"
#include "savedata.hpp"
#include "texture_port.hpp"

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#else
#include <elf.h>
#include <link.h>
#endif

namespace fs = std::filesystem;

namespace {

constexpr char kMagic[4] = {'D', 'C', 'S', 'T'};
// 7: the script VM's stacks and function records moved into arena blocks. A state from before
// holds interpreters that name the saving session's heap, which crashes the first event resumed.
constexpr std::uint32_t kVersion = 7;
constexpr std::uint32_t kOldestVersion = 7;

std::uint64_t Fingerprint(const unsigned char *bytes, std::size_t size) {
    std::uint64_t hash = 0xcbf29ce484222325;
    for (std::size_t i = 0; i < size; i++) {
        hash = (hash ^ bytes[i]) * 0x100000001b3;
    }
    return hash;
}

// A writable range of the running image, as the state carries it: an offset from the image's base.
struct StateSegment {
    std::uint64_t offset = 0;
    std::uint64_t size = 0;
};

// The running module: its base (the runtime address the layout table's offsets measure from) and
// its span. On POSIX the base is the load bias plus the lowest program header, which is the same
// point the link map measured from; on Windows the module's own base.
struct ModuleInfo {
    std::uint64_t base = 0;
    std::uint64_t span = 0;
};

#ifndef _WIN32
struct ElfScan {
    const void *self = nullptr;
    std::uint64_t base = 0;
    std::uint64_t span = 0;
};

int CollectElfModule(dl_phdr_info *info, std::size_t, void *out) {
    auto *scan = static_cast<ElfScan *>(out);
    for (int i = 0; i < info->dlpi_phnum; i++) {
        const ElfW(Phdr) &phdr = info->dlpi_phdr[i];
        if (phdr.p_type != PT_LOAD) {
            continue;
        }
        std::uintptr_t start = static_cast<std::uintptr_t>(info->dlpi_addr) + phdr.p_vaddr;
        std::uintptr_t end = start + phdr.p_memsz;
        if (reinterpret_cast<std::uintptr_t>(scan->self) < start || reinterpret_cast<std::uintptr_t>(scan->self) >= end) {
            continue;
        }
        for (int j = 0; j < info->dlpi_phnum; j++) {
            if (info->dlpi_phdr[j].p_type != PT_LOAD) {
                continue;
            }
            std::uint64_t load_start = static_cast<std::uint64_t>(info->dlpi_addr) + info->dlpi_phdr[j].p_vaddr;
            scan->span = std::max<std::uint64_t>(scan->span, load_start + info->dlpi_phdr[j].p_memsz - scan->base);
            scan->base = std::min<uint64_t>(scan->base == 0 ? load_start : scan->base, load_start);
        }
        return 1;
    }
    return 0;
}
#endif

// The whole writable image, for the table-less fallback: each writable program header (or PE
// section) as an offset from the module base. On POSIX the kernel's map clips what the linker made
// read only (RELRO), which the restore must not write to.
std::vector<StateSegment> WritableSegments(const ModuleInfo &module) {
    std::vector<StateSegment> headers;
#ifdef _WIN32
    HMODULE handle = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(&WritableSegments), &handle)) {
        return headers;
    }
    auto *dos = static_cast<IMAGE_DOS_HEADER *>(static_cast<void *>(handle));
    auto *nt = reinterpret_cast<IMAGE_NT_HEADERS *>(reinterpret_cast<std::uint8_t *>(handle) + dos->e_lfanew);
    auto *section = IMAGE_FIRST_SECTION(nt);
    for (unsigned i = 0; i < nt->FileHeader.NumberOfSections; i++, section++) {
        if ((section->Characteristics & IMAGE_SCN_MEM_WRITE) == 0 || section->Misc.VirtualSize == 0) {
            continue;
        }
        headers.push_back({section->VirtualAddress, section->Misc.VirtualSize});
    }
    return headers;
#else
    struct Scan {
        const void               *self;
        std::vector<StateSegment> *segments;
    };
    Scan scan = {reinterpret_cast<const void *>(&GameStateLoad), &headers};
    dl_iterate_phdr(
        [](dl_phdr_info *info, std::size_t, void *out) -> int {
            auto *scan = static_cast<Scan *>(out);
            for (int i = 0; i < info->dlpi_phnum; i++) {
                const ElfW(Phdr) &phdr = info->dlpi_phdr[i];
                if (phdr.p_type != PT_LOAD || (phdr.p_flags & PF_W) == 0) {
                    continue;
                }
                std::uintptr_t start = static_cast<std::uintptr_t>(info->dlpi_addr) + phdr.p_vaddr;
                std::uintptr_t end = start + phdr.p_memsz;
                if (reinterpret_cast<std::uintptr_t>(scan->self) < start ||
                    reinterpret_cast<std::uintptr_t>(scan->self) >= end) {
                    continue;
                }
                scan->segments->push_back({start - static_cast<std::uintptr_t>(info->dlpi_addr), phdr.p_memsz});
                return 1;
            }
            return 0;
        },
        &scan);
    if (headers.empty() || headers.front().size == 0) {
        return headers;
    }
    // The kernel's map, for the parts of those ranges that are writable now.
    std::uint64_t from = headers.front().offset;
    std::uint64_t to = 0;
    for (const StateSegment &header : headers) {
        to = std::max(to, header.offset + header.size);
    }
    std::FILE *maps = std::fopen("/proc/self/maps", "r");
    if (maps == nullptr) {
        return headers;
    }
    std::vector<StateSegment> clipped;
    char line[512];
    while (std::fgets(line, sizeof line, maps) != nullptr) {
        std::uint64_t start = 0;
        std::uint64_t end = 0;
        char perms[8] = {};
        if (std::sscanf(line, "%lx-%lx %7s", &start, &end, perms) != 3) {
            continue;
        }
        if (std::strchr(perms, 'w') == nullptr || start >= to || end <= from) {
            continue;
        }
        start = std::max(start, from);
        end = std::min(end, to);
        for (const StateSegment &header : headers) {
            std::uint64_t header_start = header.offset;
            std::uint64_t header_end = header.offset + header.size;
            if (start < header_end && header_start < end) {
                clipped.push_back({std::max(start, header_start) - from, std::min(end, header_end) - std::max(start, header_start)});
            }
        }
    }
    std::fclose(maps);
    return clipped.empty() ? headers : clipped;
#endif
}

ModuleInfo Module() {
#ifdef _WIN32
    HMODULE handle = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(&Module), &handle)) {
        return {};
    }
    auto *dos = static_cast<IMAGE_DOS_HEADER *>(static_cast<void *>(handle));
    auto *nt = reinterpret_cast<IMAGE_NT_HEADERS *>(reinterpret_cast<std::uint8_t *>(handle) + dos->e_lfanew);
    return {reinterpret_cast<std::uint64_t>(handle), nt->OptionalHeader.SizeOfImage};
#else
    ElfScan scan;
    scan.self = reinterpret_cast<const void *>(&Module);
    dl_iterate_phdr(CollectElfModule, &scan);
    return {scan.base, scan.span};
#endif
}

// The build's layout table: which offsets from the image base hold the game's own globals. It ships
// beside the executable (PC) and in the APK's assets (Android), both of which SDL resolves.
struct LayoutTable {
    std::uint64_t fingerprint = 0;
    std::uint64_t image_size = 0;
    std::vector<StateSegment> game;
};

LayoutTable ReadLayout() {
    LayoutTable table;
    SDL_IOStream *io = SDL_IOFromFile("states_layout.bin", "rb");
    if (io == nullptr) {
        // Not in the assets: beside the executable, where a PC build writes it. SDL owns the path.
        const char *base = SDL_GetBasePath();
        if (base != nullptr) {
            std::string path = std::string(base) + "states_layout.bin";
            io = SDL_IOFromFile(path.c_str(), "rb");
        }
    }
    if (io == nullptr) {
        return table;
    }
    size_t size = 0;
    unsigned char *bytes = static_cast<unsigned char *>(SDL_LoadFile_IO(io, &size, 0));
    SDL_CloseIO(io);
    if (bytes == nullptr || size < 24 || std::memcmp(bytes, "DCLR", 4) != 0) {
        SDL_free(bytes);
        return table;
    }
    table.fingerprint = Fingerprint(bytes, size);
    std::uint64_t lowest = 0;
    std::memcpy(&lowest, bytes + 8, 8);
    std::memcpy(&table.image_size, bytes + 16, 8);
    std::uint32_t count = 0;
    std::memcpy(&count, bytes + 24, 4);
    if (size < 28 + static_cast<size_t>(count) * 16) {
        SDL_free(bytes);
        table.fingerprint = 0;
        return table;
    }
    for (std::uint32_t i = 0; i < count; i++) {
        std::uint64_t offset = 0;
        std::uint64_t range_size = 0;
        std::memcpy(&offset, bytes + 28 + i * 16, 8);
        std::memcpy(&range_size, bytes + 28 + i * 16 + 8, 8);
        table.game.push_back({offset, range_size});
    }
    SDL_free(bytes);
    return table;
}

void Put(std::FILE *file, const void *bytes, std::size_t size, std::string *error) {
    if (error->empty() && std::fwrite(bytes, 1, size, file) != size) {
        *error = std::strerror(errno);
    }
}

void PutValue(std::FILE *file, std::uint64_t value, std::string *error) {
    Put(file, &value, sizeof value, error);
}

bool Get(std::FILE *file, void *bytes, std::size_t size) {
    return std::fread(bytes, 1, size, file) == size;
}

bool GetValue(std::FILE *file, std::uint64_t *value) {
    return Get(file, value, sizeof *value);
}

// Shifts every 8-byte value that named the saved image by the base difference: a vtable pointer, a
// static string, a function pointer. Nothing else in the game's memory lands in the image's range
// (the game's numbers are small, its doubles far from a code address), so the scan is exact.
// `origin` is the image or arena address of bytes[0]: layout-table ranges can start 4-byte-aligned,
// and a scan from the buffer's start would miss every real pointer and rewrite the wrong slots.
std::size_t Rebase(unsigned char *bytes, std::size_t size, std::uint64_t old_base, std::uint64_t new_base,
                   std::uint64_t image_size, std::uint64_t origin) {
    std::uint64_t delta = new_base - old_base;
    std::size_t   rebased = 0;
    std::size_t   i = (origin & 7) ? 8 - (origin & 7) : 0;
    for (; i + 8 <= size; i += 8) {
        std::uint64_t value = 0;
        std::memcpy(&value, bytes + i, 8);
        if (value >= old_base && value - old_base < image_size) {
            value += delta;
            std::memcpy(bytes + i, &value, 8);
            rebased++;
        }
    }
    return rebased;
}

} // namespace

bool GameStateSave(const std::filesystem::path &path, std::string *error) {
    ModuleInfo module = Module();
    if (module.span == 0) {
        *error = "no code image found";
        return false;
    }
    if (ArenaRegionBase() == nullptr) {
        *error = "no fixed arena reservation, so a state could not be restored";
        return false;
    }
    LayoutTable layout = ReadLayout();
    std::vector<StateSegment> chunks;
    if (layout.fingerprint != 0) {
        chunks = layout.game;
    } else {
        // No table: the whole writable image, good for this session only.
        chunks = WritableSegments(module);
    }
    if (chunks.empty()) {
        *error = "no writable image ranges found";
        return false;
    }
    std::FILE *file = std::fopen(path.string().c_str(), "wb");
    if (file == nullptr) {
        *error = std::strerror(errno);
        return false;
    }
    error->clear();
    Put(file, kMagic, sizeof kMagic, error);
    PutValue(file, kVersion, error);
    PutValue(file, layout.fingerprint, error);
    PutValue(file, module.base, error);
    PutValue(file, layout.fingerprint != 0 ? layout.image_size : module.span, error);
    PutValue(file, ArenaRegionBase() != nullptr ? reinterpret_cast<std::uint64_t>(ArenaRegionBase()) : 0, error);
    PutValue(file, chunks.size(), error);
    std::vector<std::vector<unsigned char>> contents;
    for (const StateSegment &chunk : chunks) {
        PutValue(file, chunk.offset, error);
        PutValue(file, chunk.size, error);
        std::vector<unsigned char> bytes(chunk.size);
        std::memcpy(bytes.data(), reinterpret_cast<const void *>(module.base + chunk.offset), chunk.size);
        Put(file, bytes.data(), bytes.size(), error);
        contents.push_back(std::move(bytes));
        if (!error->empty()) {
            break;
        }
    }
    std::uint64_t blocks = ArenaBlockCount();
    PutValue(file, blocks, error);
    for (std::uint64_t i = 0; i < blocks && error->empty(); i++) {
        ArenaBlockInfo block = ArenaBlockInfoAt(static_cast<std::size_t>(i));
        std::uint64_t owner_offset = reinterpret_cast<std::uintptr_t>(block.owner) - module.base;
        PutValue(file, owner_offset, error);
        PutValue(file, block.offset, error);
        PutValue(file, block.usable, error);
        PutValue(file, block.lead, error);
        Put(file, ArenaRegionBase() + block.offset, block.usable, error);
    }
    std::uint64_t chunk_bytes = 0;
    for (const auto &bytes : contents) {
        chunk_bytes += bytes.size();
    }
    // The models' built render records: the only copy of their geometry exists here and in the
    // running session — the game's model data is a scratch buffer long overwritten — so the state
    // carries the records whole, at their block's offset in the reservation.
    auto lasting = Draw3DLastingVisuals();
    std::vector<std::pair<const Draw3DVisual *, std::uint64_t>> records;
    std::uint64_t region = reinterpret_cast<std::uint64_t>(ArenaRegionBase());
    std::uint64_t region_size = ArenaRegionSize();
    std::uint64_t visual_vertices = 0;
    for (const auto &[block, visual] : lasting) {
        std::uint64_t offset = reinterpret_cast<std::uintptr_t>(block) - region;
        if (offset >= region_size) {
            continue;
        }
        visual_vertices += visual->vertices.size();
        records.push_back({visual, offset});
    }
    std::fprintf(stderr,
                 "state save: table %s, %llu chunk bytes, %llu blocks, %zu visuals (%llu vertices), save data %zu, region %p, image base %llx\n",
                 layout.fingerprint != 0 ? "on" : "off", static_cast<unsigned long long>(chunk_bytes),
                 static_cast<unsigned long long>(blocks), records.size(),
                 static_cast<unsigned long long>(visual_vertices), sizeof(CSaveData),
                 static_cast<void *>(ArenaRegionBase()), static_cast<unsigned long long>(module.base));
    PutValue(file, records.size(), error);
    for (const auto &[visual, offset] : records) {
        PutValue(file, offset, error);
        PutValue(file, visual->vertex_colour ? 1 : 0, error);
        PutValue(file, visual->strips.size(), error);
        Put(file, visual->strips.data(), visual->strips.size() * sizeof(Draw3DStrip), error);
        PutValue(file, visual->vertices.size(), error);
        Put(file, visual->vertices.data(), visual->vertices.size() * sizeof(gfx::Vertex3D), error);
        PutValue(file, visual->indices.size(), error);
        Put(file, visual->indices.data(), visual->indices.size() * sizeof(uint32_t), error);
    }
    // Retail's save_data is a static in main.cpp, so the layout table never sees the port's
    // CSaveData: items, dungeon status, flags and Georama live here and nowhere else in a state.
    PutValue(file, sizeof(CSaveData), error);
    if (SaveData == nullptr) {
        *error = "no save data to write";
    } else {
        Put(file, SaveData, sizeof(CSaveData), error);
    }
    std::vector<PortTextureSnapshot> snapshots;
    TextureCollectSnapshots(&snapshots);
    PutValue(file, snapshots.size(), error);
    for (const PortTextureSnapshot &snap : snapshots) {
        char name[32] = {};
        std::strncpy(name, snap.name.c_str(), sizeof name - 1);
        Put(file, name, sizeof name, error);
        PutValue(file, snap.width, error);
        PutValue(file, snap.height, error);
        PutValue(file, static_cast<std::uint64_t>(snap.bpp), error);
        PutValue(file, snap.indexed ? 1 : 0, error);
        PutValue(file, snap.pixels.size(), error);
        Put(file, snap.pixels.data(), snap.pixels.size(), error);
        if (snap.indexed) {
            Put(file, snap.palette.data(), snap.palette.size() * sizeof(uint32_t), error);
        }
    }
    std::fprintf(stderr, "state save: %zu texture snapshots\n", snapshots.size());
    bool ok = std::fflush(file) == 0 && error->empty();
    std::fprintf(stderr, "state save: %lld bytes on disk\n", ok ? static_cast<long long>(std::ftell(file)) : -1LL);
    if (std::fclose(file) != 0) {
        ok = false;
        if (error->empty()) {
            *error = std::strerror(errno);
        }
    }
    return ok;
}

bool GameStateLoad(const std::filesystem::path &path, std::string *error) {
    ModuleInfo module = Module();
    std::FILE *file = std::fopen(path.string().c_str(), "rb");
    if (file == nullptr) {
        *error = std::strerror(errno);
        return false;
    }
    error->clear();
    char magic[4] = {};
    std::uint64_t version = 0;
    std::uint64_t fingerprint = 0;
    std::uint64_t old_base = 0;
    std::uint64_t image_size = 0;
    std::uint64_t old_region = 0;
    bool ok = Get(file, magic, sizeof magic) && std::memcmp(magic, kMagic, sizeof magic) == 0 &&
              GetValue(file, &version) && GetValue(file, &fingerprint) && GetValue(file, &old_base) && GetValue(file, &image_size) &&
              GetValue(file, &old_region);
    const char *section = "header";
    if (ok && (version < kOldestVersion || version > kVersion)) {
        std::fclose(file);
        *error = version < kOldestVersion ? "the state was saved by an older version of the app; save it again"
                                          : "the state was saved by a newer version of the app";
        return false;
    }
    if (ok && ArenaRegionBase() == nullptr) {
        std::fclose(file);
        *error = "no fixed arena reservation, so a state cannot be restored";
        return false;
    }
    if (ok && fingerprint != 0) {
        // The table the state was saved with must be this build's, and the reservation must have
        // landed where it did.
        LayoutTable layout = ReadLayout();
        if (layout.fingerprint != fingerprint) {
            std::fclose(file);
            *error = "the state was saved by a different build of the game";
            return false;
        }
        if (old_region != reinterpret_cast<std::uint64_t>(ArenaRegionBase())) {
            std::fclose(file);
            *error = "the arena reservation landed elsewhere in this session";
            return false;
        }
    } else if (ok) {
        // Table-less: the whole image was carried, which is only meaningful where it sat.
        if (old_base != module.base || old_region != reinterpret_cast<std::uint64_t>(ArenaRegionBase())) {
            std::fclose(file);
            *error = "the state was saved in another session";
            return false;
        }
    }
    std::uint64_t count = 0;
    if (ok) {
        ok = GetValue(file, &count);
        section = "chunks";
    }
    std::vector<StateSegment> chunks;
    std::vector<std::vector<unsigned char>> contents;
    std::uint64_t chunk_bytes = 0;
    for (std::uint64_t i = 0; ok && i < count; i++) {
        std::uint64_t offset = 0;
        std::uint64_t size = 0;
        ok = GetValue(file, &offset) && GetValue(file, &size) && size < (std::uint64_t{1} << 31);
        if (!ok) {
            break;
        }
        contents.emplace_back(size);
        StateSegment chunk = {offset, size};
        ok = Get(file, contents.back().data(), size);
        chunk_bytes += size;
        chunks.push_back(chunk);
    }
    std::fprintf(stderr, "state load parse: %llu chunks, %llu bytes at %lld\n",
                 static_cast<unsigned long long>(chunks.size()), static_cast<unsigned long long>(chunk_bytes),
                 ok ? static_cast<long long>(std::ftell(file)) : -1LL);
    std::uint64_t block_count = 0;
    if (ok) {
        ok = GetValue(file, &block_count);
        section = "blocks";
    }
    std::uint64_t block_bytes = 0;
    struct BlockRecord {
        std::uint64_t owner_offset = 0;
        std::uint64_t offset = 0;
        std::uint64_t usable = 0;
        std::uint64_t lead = 0;
    };
    std::vector<BlockRecord> blocks;
    std::vector<std::vector<unsigned char>> block_contents;
    for (std::uint64_t i = 0; ok && i < block_count; i++) {
        BlockRecord block;
        ok = GetValue(file, &block.owner_offset) && GetValue(file, &block.offset) && GetValue(file, &block.usable) &&
             GetValue(file, &block.lead);
        if (!ok) {
            break;
        }
        block_contents.emplace_back(block.usable);
        blocks.push_back(block);
        ok = Get(file, block_contents.back().data(), block.usable);
        block_bytes += block.usable;
    }
    std::fprintf(stderr, "state load parse: %llu blocks, %llu bytes at %lld\n",
                 static_cast<unsigned long long>(blocks.size()), static_cast<unsigned long long>(block_bytes),
                 ok ? static_cast<long long>(std::ftell(file)) : -1LL);
    std::uint64_t visual_count = 0;
    if (ok) {
        ok = GetValue(file, &visual_count);
        section = "visuals";
        std::fprintf(stderr, "state load parse: %llu visuals advertised\n",
                     static_cast<unsigned long long>(visual_count));
    }
    struct VisualRecord {
        std::uint64_t              offset = 0;
        bool                       vertex_colour = false;
        std::vector<Draw3DStrip>   strips;
        std::vector<gfx::Vertex3D> vertices;
        std::vector<uint32_t>      indices;
    };
    std::vector<VisualRecord> visuals;
    auto load_array = [&](auto &out, std::uint64_t count, const char *what, std::uint64_t visual) {
        if (!ok) {
            return;
        }
        if (count > (1u << 26)) {
            std::fprintf(stderr, "state load parse: visual %llu refuses %llu %s\n",
                         static_cast<unsigned long long>(visual), static_cast<unsigned long long>(count), what);
            ok = false;
            return;
        }
        out.resize(count);
        ok = Get(file, out.data(), count * sizeof(out[0]));
        if (!ok) {
            std::fprintf(stderr, "state load parse: visual %llu ran out of file in its %s\n",
                         static_cast<unsigned long long>(visual), what);
        }
    };
    for (std::uint64_t i = 0; ok && i < visual_count; i++) {
        VisualRecord  record;
        std::uint64_t colour = 0;
        std::uint64_t strips = 0;
        std::uint64_t vertices = 0;
        std::uint64_t indices = 0;
        // The save writes each array as a count then its bytes, not the three counts together.
        ok = GetValue(file, &record.offset) && GetValue(file, &colour) && GetValue(file, &strips);
        if (!ok) {
            break;
        }
        record.vertex_colour = colour != 0;
        load_array(record.strips, strips, "strips", i);
        ok = ok && GetValue(file, &vertices);
        load_array(record.vertices, vertices, "vertices", i);
        ok = ok && GetValue(file, &indices);
        load_array(record.indices, indices, "indices", i);
        if (!ok) {
            break;
        }
        visuals.push_back(std::move(record));
    }
    std::uint64_t save_size = 0;
    std::vector<unsigned char> save_bytes;
    if (ok) {
        ok = GetValue(file, &save_size);
        section = "save data";
        if (ok && save_size != sizeof(CSaveData)) {
            std::fprintf(stderr, "state load parse: save data is %llu bytes, expected %zu\n",
                         static_cast<unsigned long long>(save_size), sizeof(CSaveData));
            ok = false;
        }
        if (ok) {
            save_bytes.resize(save_size);
            ok = Get(file, save_bytes.data(), save_size);
        }
    }
    std::vector<PortTextureSnapshot> snapshots;
    if (ok && version >= 6) {
        std::uint64_t snap_count = 0;
        ok = GetValue(file, &snap_count);
        section = "textures";
        if (ok && snap_count > 256) {
            ok = false;
        }
        for (std::uint64_t i = 0; ok && i < snap_count; i++) {
            PortTextureSnapshot snap;
            char                name[32] = {};
            std::uint64_t       width = 0;
            std::uint64_t       height = 0;
            std::uint64_t       bpp = 0;
            std::uint64_t       indexed = 0;
            std::uint64_t       pixels = 0;
            ok = Get(file, name, sizeof name) && GetValue(file, &width) && GetValue(file, &height) &&
                 GetValue(file, &bpp) && GetValue(file, &indexed) && GetValue(file, &pixels);
            if (!ok || pixels > (1u << 26) || width > 4096 || height > 4096) {
                ok = false;
                break;
            }
            name[31] = 0;
            snap.name = name;
            snap.width = static_cast<unsigned>(width);
            snap.height = static_cast<unsigned>(height);
            snap.bpp = static_cast<int>(bpp);
            snap.indexed = indexed != 0;
            snap.pixels.resize(pixels);
            ok = Get(file, snap.pixels.data(), pixels);
            if (ok && snap.indexed) {
                ok = Get(file, snap.palette.data(), snap.palette.size() * sizeof(uint32_t));
            }
            if (ok) {
                snapshots.push_back(std::move(snap));
            }
        }
    }
    long long at = std::ftell(file);
    std::fprintf(stderr, "state load parse: %llu visuals, save data %zu, %zu texture snapshots at %lld\n",
                 static_cast<unsigned long long>(visuals.size()), save_bytes.size(), snapshots.size(), at);
    std::fclose(file);
    if (!ok) {
        if (error->empty()) {
            *error = "the state's " + std::string(section) + " section is short or wrongly shaped (at " +
                     std::to_string(at) + " bytes)";
        }
        return false;
    }
    CSaveData *live = SaveData;
    if (live == nullptr) {
        *error = "no save data to restore into";
        return false;
    }

    // Blocks first — the globals point into them — then the other blocks zeroed, then the globals,
    // with the pointers that named the saved image moved by the base difference.
    std::size_t   rebased = 0;
    std::fprintf(stderr, "state load: fingerprint %016llx, %zu chunks, %zu blocks, %zu visuals, image base %llx -> %llx\n",
                 static_cast<unsigned long long>(fingerprint), chunks.size(), blocks.size(), visuals.size(),
                 static_cast<unsigned long long>(old_base), static_cast<unsigned long long>(module.base));
    for (std::size_t i = 0; i < blocks.size(); i++) {
        ArenaBlockRestore(reinterpret_cast<const void *>(module.base + blocks[i].owner_offset), blocks[i].offset,
                          blocks[i].usable, blocks[i].lead, block_contents[i].data(), block_contents[i].size());
        if (fingerprint != 0) {
            rebased += Rebase(ArenaRegionBase() + blocks[i].offset, blocks[i].usable, old_base, module.base, image_size,
                              reinterpret_cast<std::uint64_t>(ArenaRegionBase()) + blocks[i].offset);
        }
    }
    std::vector<const void *> keep;
    for (const BlockRecord &block : blocks) {
        keep.push_back(reinterpret_cast<const void *>(module.base + block.owner_offset));
    }
    ArenaZeroOtherBlocks(keep.data(), keep.size());
    for (std::size_t i = 0; i < chunks.size(); i++) {
        if (fingerprint != 0) {
            rebased += Rebase(contents[i].data(), contents[i].size(), old_base, module.base, image_size,
                              old_base + chunks[i].offset);
        }
        std::memcpy(reinterpret_cast<void *>(module.base + chunks[i].offset), contents[i].data(), contents[i].size());
    }
    // The pointer the chunks restored may name the unused retail static; this session's save is
    // the port's object, which the layout table does not carry.
    std::memcpy(live, save_bytes.data(), save_bytes.size());
    SaveData = live;
    // The game's memory says every model's VU data is built, but this session's records of them
    // are gone: rebuilt from the records the state carried, after the textures their materials
    // name. The blocks already hold their tags.
    TextureRebuild(snapshots);
    Draw3DClearVisuals();
    std::size_t vertices = 0;
    for (const VisualRecord &record : visuals) {
        Draw3DVisual &visual = Draw3DRegisterVisual(
            reinterpret_cast<unsigned int *>(reinterpret_cast<std::uint64_t>(ArenaRegionBase()) + record.offset), false);
        visual.vertex_colour = record.vertex_colour;
        visual.strips = record.strips;
        visual.vertices = record.vertices;
        visual.indices = record.indices;
        vertices += visual.vertices.size();
        Draw3DFinishVisual(visual);
    }
    std::fprintf(stderr, "state load: %zu pointers rebased, %zu visuals rebuilt (%zu vertices)\n", rebased,
                 visuals.size(), vertices);
    return true;
}

// ---- Requests from the app's menu -------------------------------------------------------------

namespace {

struct StateRequest {
    bool save = false;
    int  slot = 0;
};

std::vector<StateRequest> g_requests;

fs::path StatePath(int slot) {
    return std::filesystem::path("states") / ("slot-" + std::to_string(slot) + ".dcst");
}

void Report(bool save, int slot, bool ok, const std::string &error, const fs::path &last_path) {
    // last.json is for the menu, stderr for the log the player shares.
    std::fprintf(stderr, "state %s slot %d: %s%s\n", save ? "save" : "load", slot, ok ? "ok" : "failed",
                 ok ? "" : (": " + error).c_str());
    std::FILE *file = std::fopen(last_path.string().c_str(), "wb");
    if (file != nullptr) {
        std::fprintf(file, "{\"%s\": %d, \"slot\": %d, \"ok\": %s", save ? "save" : "load", slot, slot,
                     ok ? "true" : "false");
        if (!ok) {
            std::fprintf(file, ", \"error\": \"");
            for (char c : error) {
                std::fprintf(file, "%c", c == '"' || c == '\\' ? ' ' : c);
            }
            std::fprintf(file, "\"");
        }
        std::fprintf(file, "}\n");
        std::fclose(file);
    }
}

} // namespace

void GameStateRequest(bool save, int slot) {
    g_requests.push_back({save, slot});
}

bool GameStateTakeRequest(bool *save, int *slot) {
    if (g_requests.empty()) {
        return false;
    }
    *save = g_requests.front().save;
    *slot = g_requests.front().slot;
    g_requests.erase(g_requests.begin());
    return true;
}

bool GameStateHasPending() {
    return !g_requests.empty();
}

bool GameStateRunRequest(const std::filesystem::path &save_root, bool save, int slot) {
    fs::path path = save_root / StatePath(slot);
    std::error_code unused;
    fs::create_directories(path.parent_path(), unused);
    std::string error;
    bool ok = save ? GameStateSave(path, &error) : GameStateLoad(path, &error);
    Report(save, slot, ok, error, path.parent_path() / "last.json");
    return ok;
}