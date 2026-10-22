#include "state_save.hpp"

#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <vector>

#include "platform/arena.hpp"
#include "platform/memory.hpp"

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#else
#include <elf.h>
#include <link.h>
#endif

namespace {

constexpr char kMagic[4] = {'D', 'C', 'S', 'T'};
constexpr std::uint32_t kVersion = 1;

// A writable range of the running image: the game's globals and the port's own statics together.
struct StateSegment {
    const void *address = nullptr;
    std::size_t size = 0;
};

// The ranges actually writable now, clipped from the module's writable program headers: the
// dynamic linker keeps a part of the first one read only (RELRO), and only /proc's map says which
// part. Ordering follows the file, so a state restores in the same layout it captured.
std::vector<StateSegment> WritableSegmentsFromMaps(std::uintptr_t from, std::uintptr_t to) {
    std::vector<StateSegment> segments;
    std::FILE                *maps = std::fopen("/proc/self/maps", "r");
    if (maps == nullptr) {
        return segments;
    }
    char line[512];
    while (std::fgets(line, sizeof line, maps) != nullptr) {
        std::uintptr_t start = 0;
        std::uintptr_t end = 0;
        char           perms[8] = {};
        if (std::sscanf(line, "%lx-%lx %7s", &start, &end, perms) != 3) {
            continue;
        }
        if (std::strchr(perms, 'w') == nullptr || start >= to || end <= from) {
            continue;
        }
        start = std::max(start, from);
        end = std::min(end, to);
        segments.push_back({reinterpret_cast<const void *>(start), end - start});
    }
    std::fclose(maps);
    return segments;
}

#ifndef _WIN32
// This module's writable program headers: the ranges the image owns, before the linker's
// protections clip them.
struct ElfScan {
    std::uintptr_t self = 0;
    std::vector<StateSegment> *segments = nullptr;
};

int CollectElfSegments(dl_phdr_info *info, std::size_t, void *out) {
    auto *scan = static_cast<ElfScan *>(out);
    for (int i = 0; i < info->dlpi_phnum; i++) {
        const ElfW(Phdr) &phdr = info->dlpi_phdr[i];
        if (phdr.p_type != PT_LOAD || (phdr.p_flags & PF_W) == 0) {
            continue;
        }
        std::uintptr_t start = info->dlpi_addr + phdr.p_vaddr;
        std::uintptr_t end = start + phdr.p_memsz;
        if (scan->self < start || scan->self >= end) {
            continue;
        }
        scan->segments->push_back({reinterpret_cast<const void *>(start), phdr.p_memsz});
        return 1;
    }
    return 0;
}
#endif

// The image's writable ranges. Anything else that lives in them (the host's statics, the
// renderer's globals) is captured and restored wholesale: the save and the load both run between
// ticks, where the host holds still, and the handles it keeps name objects of this very process.
std::vector<StateSegment> WritableSegments() {
    std::vector<StateSegment> headers;
#ifdef _WIN32
    HMODULE module = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(&WritableSegments), &module)) {
        return headers;
    }
    auto *dos = static_cast<IMAGE_DOS_HEADER *>(static_cast<void *>(module));
    auto *nt = reinterpret_cast<IMAGE_NT_HEADERS *>(reinterpret_cast<std::uint8_t *>(module) + dos->e_lfanew);
    auto *section = IMAGE_FIRST_SECTION(nt);
    std::uintptr_t base = reinterpret_cast<std::uintptr_t>(module);
    for (unsigned i = 0; i < nt->FileHeader.NumberOfSections; i++, section++) {
        if ((section->Characteristics & IMAGE_SCN_MEM_WRITE) == 0 || section->Misc.VirtualSize == 0) {
            continue;
        }
        headers.push_back({reinterpret_cast<const void *>(base + section->VirtualAddress), section->Misc.VirtualSize});
    }
#else
    // The module this file is linked into: on Android libmain.so, here the test or game executable.
    ElfScan                   scan = {reinterpret_cast<std::uintptr_t>(&WritableSegments), &headers};
    dl_iterate_phdr(CollectElfSegments, &scan);
    return headers;
#endif
}

#ifndef _WIN32
// The parts of those ranges the kernel reports writable now, RELRO aside. Each map range is
// clipped against every header, since a module can carry more than one writable load.
std::vector<StateSegment> WritableSegmentsNow(const std::vector<StateSegment> &headers) {
    if (headers.empty()) {
        return {};
    }
    std::uintptr_t from = reinterpret_cast<std::uintptr_t>(headers.front().address);
    std::uintptr_t to = from;
    for (const StateSegment &header : headers) {
        std::uintptr_t start = reinterpret_cast<std::uintptr_t>(header.address);
        from = std::min(from, start);
        to = std::max(to, start + header.size);
    }
    std::vector<StateSegment> clipped = WritableSegmentsFromMaps(from, to);
    if (clipped.empty()) {
        // No /proc here: take the headers whole and let a write fault speak for itself.
        return headers;
    }
    std::vector<StateSegment> out;
    for (const StateSegment &range : clipped) {
        std::uintptr_t start = reinterpret_cast<std::uintptr_t>(range.address);
        std::uintptr_t end = start + range.size;
        for (const StateSegment &header : headers) {
            std::uintptr_t header_start = reinterpret_cast<std::uintptr_t>(header.address);
            std::uintptr_t header_end = header_start + header.size;
            if (start < header_end && header_start < end) {
                out.push_back({reinterpret_cast<const void *>(std::max(start, header_start)),
                               std::min(end, header_end) - std::max(start, header_start)});
            }
        }
    }
    return out;
}
#endif

std::vector<StateSegment> Segments() {
#ifdef _WIN32
    return WritableSegments();
#else
    return WritableSegmentsNow(WritableSegments());
#endif
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

} // namespace

bool GameStateSave(const std::filesystem::path &path, std::string *error) {
    std::vector<StateSegment> segments = Segments();
    if (segments.empty()) {
        *error = "no writable image segments found";
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
    PutValue(file, segments.size(), error);
    for (const StateSegment &segment : segments) {
        PutValue(file, reinterpret_cast<std::uintptr_t>(segment.address), error);
        PutValue(file, segment.size, error);
        Put(file, segment.address, segment.size, error);
    }
    std::uint64_t blocks = ArenaBlockCount();
    PutValue(file, blocks, error);
    for (std::uint64_t i = 0; i < blocks && error->empty(); i++) {
        ArenaBlockInfo block = ArenaBlockInfoAt(static_cast<std::size_t>(i));
        PutValue(file, reinterpret_cast<std::uintptr_t>(block.owner), error);
        PutValue(file, reinterpret_cast<std::uintptr_t>(block.map), error);
        PutValue(file, block.map_size, error);
        PutValue(file, block.lead, error);
        Put(file, block.map, block.map_size - ArenaMemoryPageSize(), error);
    }
    bool ok = std::fflush(file) == 0 && error->empty();
    if (std::fclose(file) != 0) {
        ok = false;
        if (error->empty()) {
            *error = std::strerror(errno);
        }
    }
    return ok;
}

bool GameStateLoad(const std::filesystem::path &path, std::string *error) {
    std::FILE *file = std::fopen(path.string().c_str(), "rb");
    if (file == nullptr) {
        *error = std::strerror(errno);
        return false;
    }
    error->clear();
    std::vector<StateSegment> segments;
    std::uint64_t             version = 0;
    char                      magic[4] = {};
    bool                      ok = Get(file, magic, sizeof magic) && std::memcmp(magic, kMagic, sizeof magic) == 0 &&
              GetValue(file, &version) && version == kVersion;
    std::uint64_t count = 0;
    if (ok) {
        ok = GetValue(file, &count);
    }
    std::vector<std::vector<unsigned char>> contents;
    for (std::uint64_t i = 0; ok && i < count; i++) {
        std::uint64_t address = 0;
        std::uint64_t size = 0;
        ok = GetValue(file, &address) && GetValue(file, &size) && size < (1u << 31);
        if (!ok) {
            break;
        }
        contents.emplace_back(size);
        StateSegment segment = {reinterpret_cast<const void *>(address), size};
        ok = Get(file, contents.back().data(), size);
        segments.push_back(segment);
    }
    std::vector<std::vector<unsigned char>> block_contents;
    std::uint64_t                           block_count = 0;
    if (ok) {
        ok = GetValue(file, &block_count);
    }
    struct BlockRecord {
        const void   *owner = nullptr;
        unsigned char *map = nullptr;
        std::size_t    map_size = 0;
        std::size_t    lead = 0;
    };
    std::vector<BlockRecord> blocks;
    for (std::uint64_t i = 0; ok && i < block_count; i++) {
        std::uint64_t owner = 0;
        std::uint64_t map = 0;
        std::uint64_t map_size = 0;
        std::uint64_t lead = 0;
        ok = GetValue(file, &owner) && GetValue(file, &map) && GetValue(file, &map_size) && GetValue(file, &lead);
        if (!ok) {
            break;
        }
        std::size_t page = ArenaMemoryPageSize();
        if (map_size <= page || lead > map_size - page) {
            ok = false;
            break;
        }
        std::size_t usable = map_size - page;
        block_contents.emplace_back(usable);
        blocks.push_back({reinterpret_cast<const void *>(owner), reinterpret_cast<unsigned char *>(map), map_size, lead});
        ok = Get(file, block_contents.back().data(), usable);
    }
    std::fclose(file);
    if (!ok) {
        if (error->empty()) {
            *error = "the state file is too short or wrongly shaped";
        }
        return false;
    }

    // Blocks first (the image's globals point into them), then the other blocks zeroed, then the
    // image's globals over what the game's logic reads.
    for (std::size_t i = 0; i < blocks.size(); i++) {
        const BlockRecord &block = blocks[i];
        ArenaBlockRestore(block.owner, block.map, block.map_size, block.lead, block_contents[i].data(),
                          block_contents[i].size());
    }
    std::vector<const void *> keep;
    for (const BlockRecord &block : blocks) {
        keep.push_back(block.owner);
    }
    ArenaZeroOtherBlocks(keep.data(), keep.size());
    for (std::size_t i = 0; i < segments.size(); i++) {
        std::memcpy(const_cast<void *>(segments[i].address), contents[i].data(), contents[i].size());
    }
    return true;
}