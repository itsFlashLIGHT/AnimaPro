#!/usr/bin/env python3
"""Generate AnimaPro's window icon and macOS icon using the standard library."""
import math
import pathlib
import struct
import sys
import zlib

root = pathlib.Path(__file__).resolve().parent.parent


def pixels(size):
    data = bytearray()
    for y in range(size):
        for x in range(size):
            px, py = (x + 0.5) / size, (y + 0.5) / size
            # Rounded violet tile with a warm circular character and play symbol.
            dx = max(0.14 - px, 0.0, px - 0.86)
            dy = max(0.14 - py, 0.0, py - 0.86)
            if math.hypot(dx, dy) > 0.14:
                data.extend((0, 0, 0, 0))
            elif (px - 0.5) ** 2 + (py - 0.5) ** 2 < 0.31**2:
                if 0.43 <= px <= 0.65 and abs(py - 0.5) <= (0.65 - px) * 0.66:
                    data.extend((255, 255, 255, 255))
                else:
                    data.extend((255, 157, 76, 255))
            else:
                data.extend((61, 65, 151, 255))
    return bytes(data)


def png(size, rgba):
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    rows = b"".join(b"\0" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


outputs = {"icon.rgba": pixels(64)}
entries = bytearray()
for kind, size in ((b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024)):
    image = png(size, pixels(size))
    entries += kind + struct.pack(">I", len(image) + 8) + image
outputs["AnimaPro.icns"] = b"icns" + struct.pack(">I", len(entries) + 8) + entries

# The Windows resource format can contain PNG-compressed icon images directly.
images = [(size, png(size, pixels(size))) for size in (16, 32, 64, 128, 256)]
directory = bytearray(struct.pack("<HHH", 0, 1, len(images)))
offset = 6 + 16 * len(images)
group = bytearray(struct.pack("<HHH", 0, 1, len(images)))
for index, (size, image) in enumerate(images, 1):
    dimensions = 0 if size == 256 else size
    common = struct.pack("<BBBBHHI", dimensions, dimensions, 0, 0, 1, 32, len(image))
    directory += common + struct.pack("<I", offset)
    group += common + struct.pack("<H", index)
    offset += len(image)
outputs["AnimaPro.ico"] = directory + b"".join(image for _, image in images)


def resource(kind, ident, data):
    header = struct.pack("<IIHHHHIHHII", len(data), 32, 0xffff, kind, 0xffff, ident, 0, 0x1030, 0x0409, 0, 0)
    return header + data + b"\0" * (-len(data) % 4)


# The file starts with the conventional empty resource header.
compiled = struct.pack("<IIHHHHIHHII", 0, 32, 0xffff, 0, 0xffff, 0, 0, 0, 0, 0, 0)
for index, (_, image) in enumerate(images, 1):
    compiled += resource(3, index, image)  # RT_ICON
compiled += resource(14, 1, group)  # RT_GROUP_ICON
outputs["AnimaPro.res"] = compiled

if sys.argv[1:] not in ([], ["--check"]):
    raise SystemExit("Usage: create-icons.py [--check]")
for name, content in outputs.items():
    target = root / "assets" / name
    if sys.argv[1:]:
        if not target.is_file() or target.read_bytes() != content:
            raise SystemExit(f"Stale icon: {target}. Run scripts/create-icons.py")
    else:
        target.write_bytes(content)
