// What the Android setup screen (android/app/.../SetupActivity.java, through android/src/setup.rs)
// asks of the disc before the game starts: what it is, and its data extracted, by the same
// FirstRunExtract the in-game first-run screen uses.

#ifdef __ANDROID__

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <exception>
#include <format>
#include <string>

#include "firstrun.hpp"
#include "platform/paths.hpp"

namespace {

void CopyOut(const std::string &text, char *out, std::size_t size) {
    if (out == nullptr || size == 0) {
        return;
    }
    std::size_t length = std::min(text.size(), size - 1);
    std::memcpy(out, text.data(), length);
    out[length] = '\0';
}

std::string Escaped(std::string_view text) {
    std::string escaped;
    for (char c : text) {
        if (c == '"' || c == '\\') {
            escaped += '\\';
            escaped += c;
        } else if (static_cast<unsigned char>(c) < 0x20) {
            escaped += std::format("\\u{:04x}", c);
        } else {
            escaped += c;
        }
    }
    return escaped;
}

} // namespace

// The disc as JSON: {"serial", "region", "release", "languages", "files", "bytes"} or {"error"}.
extern "C" bool chronicle_identify_disc(const char *source, char *json, std::size_t size) {
    try {
        dcdata::DiscInfo info = dcdata::IdentifyDisc(PathsFromUtf8(source));
        std::string      languages;
        for (int language : info.release.languages) {
            languages += std::format("{}{}", languages.empty() ? "" : ",", language);
        }
        CopyOut(std::format("{{\"serial\":\"{}\",\"region\":\"{}\",\"release\":\"{}\",\"languages\":[{}],"
                            "\"files\":{},\"bytes\":{}}}",
                            Escaped(info.serial), Escaped(dcdata::RegionName(info.region)),
                            Escaped(info.release.name), languages, info.files, info.bytes),
                json, size);
        return true;
    } catch (const std::exception &failure) {
        CopyOut(std::format("{{\"error\":\"{}\"}}", Escaped(failure.what())), json, size);
        return false;
    }
}

using ChronicleExtractTick = bool (*)(void *user, std::uint64_t files, std::uint64_t total_files,
                                      std::uint64_t bytes, std::uint64_t total_bytes);

// Extracts source into root, calling tick with the progress on this thread; tick returning false
// cancels. False with the reason in error on failure or cancel.
extern "C" bool chronicle_extract_disc(const char *source, const char *root, ChronicleExtractTick tick, void *user,
                                       char *error, std::size_t size) {
    FirstRunOutcome outcome =
        FirstRunExtract(PathsFromUtf8(source), PathsFromUtf8(root), [&](const dcdata::Progress &progress) {
            return tick == nullptr || tick(user, progress.files, progress.total_files, progress.bytes,
                                           progress.total_bytes);
        });
    if (!outcome.ok) {
        CopyOut(outcome.cancelled ? std::string("cancelled") : outcome.error, error, size);
    }
    return outcome.ok;
}

#endif
