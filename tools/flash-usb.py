#!/usr/bin/env python
"""Raw-disk flasher for the ClaudioOS UEFI image.

Writes a raw FAT32 UEFI image to \\\\.\\PHYSICALDRIVEn.
Must run as admin. The target disk must be offline / have no open handles —
use `diskpart > select disk N > offline disk` first, or pass --offline to
take it offline automatically.

Usage:
    python flash-usb.py <image> <physical-drive-number> [--offline]
"""
import argparse
import ctypes
import os
import subprocess
import sys
import time
from ctypes import wintypes

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

# Win32 constants for CreateFile / raw disk I/O.
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
OPEN_EXISTING = 3
FILE_ATTRIBUTE_NORMAL = 0x80
FILE_FLAG_NO_BUFFERING = 0x20000000
FILE_FLAG_WRITE_THROUGH = 0x80000000
FSCTL_LOCK_VOLUME = 0x00090018
FSCTL_DISMOUNT_VOLUME = 0x00090020
FSCTL_ALLOW_EXTENDED_DASD_IO = 0x00090083
IOCTL_DISK_DELETE_DRIVE_LAYOUT = 0x0007c100
IOCTL_DISK_UPDATE_PROPERTIES = 0x00070140
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value


def _win_write_raw(device: str, src_path: str) -> int:
    """Open a physical drive with the right sharing flags for a raw write,
    dismount any volumes on it, then stream the source file to it.

    Returns bytes written."""
    k32 = ctypes.windll.kernel32
    k32.CreateFileW.restype = wintypes.HANDLE
    k32.CreateFileW.argtypes = [
        wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
        ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
    ]
    k32.DeviceIoControl.argtypes = [
        wintypes.HANDLE, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD,
        ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD),
        ctypes.c_void_p,
    ]
    k32.DeviceIoControl.restype = wintypes.BOOL
    k32.WriteFile.argtypes = [
        wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p,
    ]
    k32.WriteFile.restype = wintypes.BOOL

    handle = k32.CreateFileW(
        device,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
        None,
    )
    if handle == INVALID_HANDLE_VALUE or handle is None:
        err = ctypes.GetLastError()
        raise OSError(f"CreateFileW({device}) failed: {err}")

    try:
        bytes_returned = wintypes.DWORD(0)
        # Wipe the drive layout FIRST so Windows drops any phantom partition
        # info it was holding. Then dismount + lock + allow extended DASD I/O
        # (which permits writes past the filesystem end and outside mounted
        # partitions — exactly what we need for a full raw image dump).
        k32.DeviceIoControl(
            handle, IOCTL_DISK_DELETE_DRIVE_LAYOUT, None, 0, None, 0,
            ctypes.byref(bytes_returned), None,
        )
        k32.DeviceIoControl(
            handle, FSCTL_DISMOUNT_VOLUME, None, 0, None, 0,
            ctypes.byref(bytes_returned), None,
        )
        lock_ok = k32.DeviceIoControl(
            handle, FSCTL_LOCK_VOLUME, None, 0, None, 0,
            ctypes.byref(bytes_returned), None,
        )
        k32.DeviceIoControl(
            handle, FSCTL_ALLOW_EXTENDED_DASD_IO, None, 0, None, 0,
            ctypes.byref(bytes_returned), None,
        )
        if not lock_ok:
            print(f"[flash] warning: FSCTL_LOCK_VOLUME failed (err={ctypes.GetLastError()}) — continuing")

        size = os.path.getsize(src_path)
        # Write in 4 MiB chunks, sector-aligned (FILE_FLAG_NO_BUFFERING needs
        # aligned buffer sizes).
        chunk = 4 * 1024 * 1024
        written = 0
        t0 = time.time()
        written_out = wintypes.DWORD(0)
        with open(src_path, "rb") as src:
            while True:
                buf = src.read(chunk)
                if not buf:
                    break
                # Pad to sector boundary (512) if short read at EOF.
                if len(buf) % 512 != 0:
                    buf = buf + b"\x00" * (512 - (len(buf) % 512))
                # Retry loop with re-dismount between attempts. Windows'
                # volume manager periodically re-mounts the partition it sees
                # in the FAT32 bytes we just wrote, which yanks our exclusive
                # handle. Re-dismount + re-lock + retry.
                tries = 0
                while True:
                    ok = k32.WriteFile(
                        handle, buf, len(buf),
                        ctypes.byref(written_out), None,
                    )
                    if ok:
                        break
                    err = ctypes.GetLastError()
                    tries += 1
                    if err == 5 and tries < 10:
                        # ACCESS_DENIED — re-dismount + re-lock, try again.
                        k32.DeviceIoControl(
                            handle, FSCTL_DISMOUNT_VOLUME, None, 0, None, 0,
                            ctypes.byref(bytes_returned), None,
                        )
                        k32.DeviceIoControl(
                            handle, FSCTL_LOCK_VOLUME, None, 0, None, 0,
                            ctypes.byref(bytes_returned), None,
                        )
                        time.sleep(0.05)
                        continue
                    raise OSError(f"WriteFile failed at offset {written}: {err} (tries={tries})")
                written += written_out.value
                elapsed = time.time() - t0
                mb_s = (written / 1024 / 1024) / elapsed if elapsed > 0 else 0
                pct = 100 * written / size
                print(
                    f"  {written:>12,} / {size:,} bytes  ({pct:5.1f}%)  {mb_s:5.1f} MB/s",
                    flush=True,
                )
        return written
    finally:
        k32.CloseHandle(handle)


def run_diskpart(script: str) -> tuple[int, str]:
    p = subprocess.run(
        ["diskpart"],
        input=script.encode("utf-16-le")[2:] if False else script.encode(),
        capture_output=True,
        text=False,
    )
    out = (p.stdout or b"") + (p.stderr or b"")
    return p.returncode, out.decode("utf-8", errors="replace")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("image")
    ap.add_argument("drive", type=int, help="physical drive number, e.g. 6")
    ap.add_argument("--offline", action="store_true")
    ap.add_argument("--online", action="store_true")
    args = ap.parse_args()

    img = args.image
    if not os.path.exists(img):
        print(f"error: image not found: {img}", file=sys.stderr)
        return 1
    size = os.path.getsize(img)
    dev = rf"\\.\PHYSICALDRIVE{args.drive}"
    print(f"[flash] source: {img} ({size:,} bytes)")
    print(f"[flash] target: {dev}")

    if args.offline:
        print(f"[flash] taking disk {args.drive} offline via diskpart...")
        rc, out = run_diskpart(
            f"select disk {args.drive}\nofflinedisk noerr\noffline disk noerr\n"
        )
        print(out.strip())

    t0 = time.time()
    written = _win_write_raw(dev, img)
    elapsed = time.time() - t0
    print(f"[flash] wrote {written:,} bytes in {elapsed:.1f}s")

    if args.online:
        print(f"[flash] bringing disk {args.drive} online...")
        rc, out = run_diskpart(f"select disk {args.drive}\nonline disk noerr\n")
        print(out.strip())

    print("[flash] done. safe to remove the USB stick.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
