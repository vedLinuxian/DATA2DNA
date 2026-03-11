#!/usr/bin/env python3
"""Create corrupted versions of archives for Helix-Salvager testing."""
import os, random, shutil

BASE = os.path.dirname(os.path.abspath(__file__))

def corrupt(src, dst, regions):
    """Copy src to dst, then zero-out the specified byte regions."""
    shutil.copy2(src, dst)
    with open(dst, 'r+b') as f:
        total = f.seek(0, 2)
        for start_frac, length in regions:
            pos = int(total * start_frac)
            f.seek(pos)
            f.write(b'\x00' * length)
    print(f"  Created: {dst} ({os.path.getsize(dst)} bytes, {len(regions)} corrupt regions)")

# ── Source archive ──
src_7z = os.path.join(BASE, "test_photos.7z")

# ── Also create a ZIP for comparison ──
src_zip = os.path.join(BASE, "test_photos.zip")
if not os.path.exists(src_zip):
    os.system(f'cd "{BASE}" && zip -j test_photos.zip dummy_data_to_7z/*.png')

print("=== Creating corrupted test archives ===\n")

# ── Test 1: Light corruption (100 bytes zeroed in middle) ──
corrupt(src_7z,  os.path.join(BASE, "test_corrupt_light.7z"),
        [(0.35, 100)])
corrupt(src_zip, os.path.join(BASE, "test_corrupt_light.zip"),
        [(0.35, 100)])

# ── Test 2: Medium corruption (3 regions, 500 bytes each) ──
corrupt(src_7z,  os.path.join(BASE, "test_corrupt_medium.7z"),
        [(0.15, 500), (0.45, 500), (0.75, 500)])
corrupt(src_zip, os.path.join(BASE, "test_corrupt_medium.zip"),
        [(0.15, 500), (0.45, 500), (0.75, 500)])

# ── Test 3: Heavy corruption (5 regions, 2KB each = 10KB total damage) ──
corrupt(src_7z,  os.path.join(BASE, "test_corrupt_heavy.7z"),
        [(0.10, 2048), (0.25, 2048), (0.50, 2048), (0.70, 2048), (0.85, 2048)])
corrupt(src_zip, os.path.join(BASE, "test_corrupt_heavy.zip"),
        [(0.10, 2048), (0.25, 2048), (0.50, 2048), (0.70, 2048), (0.85, 2048)])

# ── Test 4: Header destroyed (first 64 bytes zeroed — kills central directory pointer for ZIP) ──
corrupt(src_7z,  os.path.join(BASE, "test_corrupt_header.7z"),
        [(0.0, 64)])
corrupt(src_zip, os.path.join(BASE, "test_corrupt_header.zip"),
        [(0.0, 64)])

# ── Test 5: Catastrophic (20KB random damage across 10 spots) ──
corrupt(src_7z,  os.path.join(BASE, "test_corrupt_catastrophic.7z"),
        [(i/12.0, 2048) for i in range(1, 11)])
corrupt(src_zip, os.path.join(BASE, "test_corrupt_catastrophic.zip"),
        [(i/12.0, 2048) for i in range(1, 11)])

print("\n=== All corrupt test files created ===")
print("\nAlso verifying that 7z cannot extract the corrupted archives:")
for name in ["test_corrupt_light.7z", "test_corrupt_medium.7z", "test_corrupt_heavy.7z"]:
    path = os.path.join(BASE, name)
    ret = os.system(f'7z t "{path}" > /dev/null 2>&1')
    status = "OK" if ret == 0 else f"FAILED (exit {ret >> 8})"
    print(f"  7z test {name}: {status}")
