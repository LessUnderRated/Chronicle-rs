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


def parse(lines):
    ranges = []
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
        if section not in WRITABLE:
            continue
        # The game's own objects: the ps2 sources CMake compiles as dc_ps2, less the Kazarov sce
        # units compiled into the same library, whose state belongs to the host session.
        if ("/dc_ps2.dir/" not in path or "/kazarov/" in path) and "/ps2/src/" not in path:
            continue
        ranges.append((vma, size))
    if lowest is None:
        raise SystemExit("game_ranges: no sections in the map")
    return lowest, max(highest - lowest, 1), ranges


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: game_ranges.py <link map> <output table>")
    with open(sys.argv[1], "r", errors="replace") as file:
        lowest, image_size, ranges = parse(file)
    merged = []
    for start, size in sorted(ranges):
        offset = start - lowest
        if merged and offset <= merged[-1][0] + merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], offset + size)
        else:
            merged.append([offset, size])
    with open(sys.argv[2], "wb") as file:
        file.write(MAGIC)
        file.write(struct.pack("<I", VERSION))
        file.write(struct.pack("<QQ", lowest, image_size))
        file.write(struct.pack("<I", len(merged)))
        for offset, size in merged:
            file.write(struct.pack("<QQ", offset, size))


if __name__ == "__main__":
    main()