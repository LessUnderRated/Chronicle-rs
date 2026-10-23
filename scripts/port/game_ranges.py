# Reads an lld ELF link map and writes the state layout table: the ranges of the game's own
# globals (.data and .bss contributions from ps2/src objects), which a save state alone restores.
# The host's own globals (the renderer's, the texture registry's, this file's) are left out: their
# content belongs to the running session, and a load keeps it.
#
# usage: game_ranges.py <link map> <output table>
# The table: "DCLR", u32 version 1, u64 image offset base (the lowest VMA), u64 image size (the
# widest span the image's sections cover), u32 range count, then per range u64 offset, u64 size,
# offsets measured from the base. A state names this table's hash, so a load refuses one saved by
# a differently laid-out build.

import struct
import sys

MAGIC = b"DCLR"
VERSION = 1
WRITABLE = (".bss", ".data")

# port/src units that replace retail's game logic whole, with retail's statics in them: an interior's
# characters and cameras (edit_in), the town loop's menu and event state (editloop, editloop3), the
# dungeon's player and status panel (dun/). Their globals are the game's as much as ps2/src's are,
# and hold nothing of the host session's (no heap containers, no renderer handles), so a state
# carries them. The rest of port/src is the host's.
PORT_GAME_UNITS = (
    "/dc_port.dir/src/edit_in.cpp.o",
    "/dc_port.dir/src/editloop.cpp.o",
    "/dc_port.dir/src/editloop3.cpp.o",
    "/dc_port.dir/src/dun/gameloop.cpp.o",
    "/dc_port.dir/src/dun/movechara.cpp.o",
)


def parse(lines):
    ranges = []
    others = []
    lowest = None
    highest = 0
    for line in lines:
        tokens = line.split()
        if len(tokens) < 5 or tokens[0].lower().startswith("vma"):
            continue
        try:
            vma = int(tokens[0], 16)
            size = int(tokens[2], 16)
        except ValueError:
            continue
        out = tokens[4]
        if out.startswith("."):
            # An output section line: track the image's span.
            lowest = vma if lowest is None else min(lowest, vma)
            highest = max(highest, vma + size)
            continue
        # An input contribution: path:(section). Only the game's objects in a writable section.
        marker = out.find(":(")
        if marker < 0 or not out.endswith(")"):
            continue
        section = out[marker + 2:-1]
        path = out[:marker].replace("\\", "/")
        # -fdata-sections gives every global its own input section (.bss.rogoY3, .data._ZN...), so
        # the prefixes are what count. .data.rel.ro and its `.local` variant are made read only
        # after relocation and never rewritten, so they are not part of a state.
        writable = section.startswith(".bss") or (section.startswith(".data") and not section.startswith(".data.rel.ro"))
        if not writable:
            continue
        # The game's own objects: the ps2 sources CMake compiles as dc_ps2, less the Kazarov sce
        # units compiled into the same library, whose state belongs to the host session. The host's
        # other globals (this module's, the renderer's, the Rust crate's) are interleaved between
        # the game's by the linker, so the table must carry the game's ranges exactly and never a
        # span that covers a gap.
        game = ("/dc_ps2.dir/" in path and "/kazarov/" not in path) or "/ps2/src/" in path
        game = game or path.endswith(PORT_GAME_UNITS)
        if not game:
            others.append((vma, size))
        else:
            ranges.append((vma, size))
    if lowest is None:
        raise SystemExit("game_ranges: no sections in the map")
    return lowest, max(highest - lowest, 1), ranges, others


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: game_ranges.py <link map> <output table>")
    with open(sys.argv[1], "r", errors="replace") as file:
        lowest, image_size, ranges, others = parse(file)
    merged = []
    for start, size in sorted(ranges):
        offset = start - lowest
        if merged and offset == merged[-1][0] + merged[-1][1]:
            # Touching: one range. A gap between the game's globals holds the host's, which a state
            # must leave alone.
            merged[-1][1] += size
        else:
            merged.append([offset, size])
    for start, size in others:
        offset = start - lowest
        for begin, span in merged:
            if offset < begin + span and begin < offset + size:
                raise SystemExit("game_ranges: the game's ranges cover a host global at %x" % (start,))
    with open(sys.argv[2], "wb") as file:
        file.write(MAGIC)
        file.write(struct.pack("<I", VERSION))
        file.write(struct.pack("<QQ", lowest, image_size))
        file.write(struct.pack("<I", len(merged)))
        for offset, size in merged:
            file.write(struct.pack("<QQ", offset, size))


if __name__ == "__main__":
    main()