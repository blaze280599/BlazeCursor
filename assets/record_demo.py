"""Records the demo GIFs/MP4 in assets/ (Windows only; BlazeCursor must be running).

A borderless window with a mock desktop covers the capture area so nothing else on screen is
recorded. The real cursor is driven with a scripted shake and composited into each frame with
DrawIconEx, so what you see is exactly what BlazeCursor does to the system cursor.

    python assets/record_demo.py demo    # arrow over a window
    python assets/record_demo.py text    # I-beam over a text field
"""
import ctypes
import math
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tkinter as tk
from ctypes import wintypes as wt
from pathlib import Path

from PIL import Image

user32, gdi32 = ctypes.windll.user32, ctypes.windll.gdi32
ctypes.windll.shcore.SetProcessDpiAwareness(2)
W, H, FPS = 800, 450, 30
OUT = Path(__file__).parent


class CURSORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wt.DWORD), ("flags", wt.DWORD), ("hCursor", wt.HANDLE), ("pt", wt.POINT)]


class ICONINFO(ctypes.Structure):
    _fields_ = [("fIcon", wt.BOOL), ("xHotspot", wt.DWORD), ("yHotspot", wt.DWORD),
                ("hbmMask", wt.HANDLE), ("hbmColor", wt.HANDLE)]


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", ctypes.c_long), ("dy", ctypes.c_long), ("mouseData", wt.DWORD), ("dwFlags", wt.DWORD),
                ("time", wt.DWORD), ("dwExtraInfo", ctypes.c_size_t)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wt.DWORD), ("mi", MOUSEINPUT)]


def move_to(x, y):
    """Moves the cursor with real (injected) mouse input; BlazeCursor listens to raw input, which
    SetCursorPos does not generate."""
    vx, vy, vw, vh = (user32.GetSystemMetrics(n) for n in (76, 77, 78, 79))  # virtual desktop
    mi = MOUSEINPUT(dx=round((x - vx) * 65535 / (vw - 1)), dy=round((y - vy) * 65535 / (vh - 1)),
                    dwFlags=0x0001 | 0x8000 | 0x4000)  # MOVE | ABSOLUTE | VIRTUALDESK
    user32.SendInput(1, ctypes.byref(INPUT(type=0, mi=mi)), ctypes.sizeof(INPUT))


user32.CopyIcon.restype = wt.HANDLE
for f, r in [("GetDC", wt.HANDLE), ("CreateCompatibleDC", wt.HANDLE), ("CreateCompatibleBitmap", wt.HANDLE),
             ("SelectObject", wt.HANDLE)]:
    getattr(user32 if f == "GetDC" else gdi32, f).restype = r


def grab(x0, y0):
    """Screen region (x0, y0, W, H) with the current cursor drawn in."""
    sdc = user32.GetDC(None)
    mdc = gdi32.CreateCompatibleDC(wt.HANDLE(sdc))
    bmp = gdi32.CreateCompatibleBitmap(wt.HANDLE(sdc), W, H)
    gdi32.SelectObject(wt.HANDLE(mdc), wt.HANDLE(bmp))
    gdi32.BitBlt(wt.HANDLE(mdc), 0, 0, W, H, wt.HANDLE(sdc), x0, y0, 0x00CC0020)  # SRCCOPY
    ci = CURSORINFO(cbSize=ctypes.sizeof(CURSORINFO))
    if user32.GetCursorInfo(ctypes.byref(ci)) and ci.hCursor:
        h = wt.HANDLE(user32.CopyIcon(wt.HANDLE(ci.hCursor)))
        ii = ICONINFO()
        user32.GetIconInfo(h, ctypes.byref(ii))
        user32.DrawIconEx(wt.HANDLE(mdc), ci.pt.x - x0 - ii.xHotspot, ci.pt.y - y0 - ii.yHotspot, h, 0, 0, 0, None, 3)
        for b in (ii.hbmMask, ii.hbmColor):
            if b:
                gdi32.DeleteObject(wt.HANDLE(b))
        user32.DestroyIcon(h)
    buf = ctypes.create_string_buffer(W * H * 4)
    hdr = (ctypes.c_uint32 * 10)(40, W, (-H) & 0xFFFFFFFF, 1 | (32 << 16), 0, 0, 0, 0, 0, 0)
    gdi32.GetDIBits(wt.HANDLE(mdc), wt.HANDLE(bmp), 0, H, buf, hdr, 0)
    gdi32.DeleteObject(wt.HANDLE(bmp))
    gdi32.DeleteDC(wt.HANDLE(mdc))
    user32.ReleaseDC(None, wt.HANDLE(sdc))
    return Image.frombuffer("RGB", (W, H), buf, "raw", "BGRX", 0, 1)


def build_window(scene):
    root = tk.Tk()
    root.overrideredirect(True)
    root.attributes("-topmost", True)
    x0 = (user32.GetSystemMetrics(0) - W) // 2
    y0 = (user32.GetSystemMetrics(1) - H) // 2
    root.geometry(f"{W}x{H}+{x0}+{y0}")
    c = tk.Canvas(root, width=W, height=H, highlightthickness=0, cursor="arrow")
    c.place(x=0, y=0)
    for y in range(H):  # background gradient
        t = y / H
        c.create_line(0, y, W, y, fill="#%02x%02x%02x" % (int(30 + 20 * t), int(42 + 22 * t), int(66 + 30 * t)))
    # Mock app window on the left.
    c.create_rectangle(40, 70, 430, 400, fill="#f4f5f7", outline="")
    c.create_rectangle(40, 70, 430, 100, fill="#dfe3e8", outline="")
    for i, col in enumerate(("#ff5f57", "#febc2e", "#28c840")):
        c.create_oval(54 + i * 20, 80, 66 + i * 20, 92, fill=col, outline="")
    for i, w in enumerate((300, 250, 330, 180, 290, 220, 310, 160)):
        c.create_rectangle(64, 125 + i * 32, 64 + w, 139 + i * 32, fill="#d3d8de", outline="")
    # Mock app window on the right with a real text field (gives the I-beam cursor).
    c.create_rectangle(470, 120, 760, 340, fill="#f4f5f7", outline="")
    c.create_rectangle(470, 120, 760, 150, fill="#dfe3e8", outline="")
    c.create_text(490, 180, text="Search", anchor="w", fill="#4a5361", font=("Segoe UI", 11))
    entry = tk.Entry(root, font=("Segoe UI", 14), relief="flat", cursor="xterm", bg="white")
    entry.place(x=490, y=200, width=250, height=110)
    title = "Shake the mouse to find it" if scene == "demo" else "Works on the text cursor too"
    c.create_text(W // 2, 36, text=title, fill="white", font=("Segoe UI Semibold", 18))
    return root, x0, y0


def script(scene, x0, y0):
    """Yields (t, x, y) cursor positions for the scene, in screen coords."""
    sx, sy = (x0 + 235, y0 + 250) if scene == "demo" else (x0 + 615, y0 + 255)
    timeline = []
    if scene == "demo":
        timeline.append((0.6, lambda u: (sx - 60 + 60 * u, sy + 20 - 20 * u)))  # gentle approach
    timeline += [
        (0.5, lambda u: (sx, sy)),  # pause
        (1.1, lambda u: (sx + 110 * math.sin(2 * math.pi * 4.2 * u * 1.1), sy)),  # shake
        (2.4, lambda u: (sx, sy)),  # hold while it shrinks back
    ]
    return timeline, sum(d for d, _ in timeline)


def main(scene):
    root, x0, y0 = build_window(scene)
    timeline, total = script(scene, x0, y0)
    frames = []

    def drive():
        time.sleep(0.6)  # let the window appear
        start = time.perf_counter()
        nxt = start
        t0 = start
        for dur, fn in timeline:
            while (t := time.perf_counter() - t0) < dur:
                x, y = fn(t / dur)
                move_to(x, y)
                if time.perf_counter() >= nxt:
                    frames.append(grab(x0, y0))
                    nxt += 1 / FPS
                time.sleep(0.004)
            t0 += dur
        root.after(0, root.destroy)

    threading.Thread(target=drive, daemon=True).start()
    root.mainloop()

    with tempfile.TemporaryDirectory() as tmp:
        for i, f in enumerate(frames):
            f.save(f"{tmp}/{i:04d}.png")
        mp4 = OUT / f"{scene}.mp4"
        gif = OUT / f"{scene}.gif"
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", f"{tmp}/%04d.png",
                        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", str(mp4)], check=True)
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", f"{tmp}/%04d.png",
                        "-vf", "fps=25,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=sierra2_4a",
                        "-loop", "0", str(gif)], check=True)
    print(f"{scene}: {len(frames)} frames, {total:.1f}s -> {gif.name}, {mp4.name}")


if __name__ == "__main__":
    assert shutil.which("ffmpeg"), "ffmpeg is required"
    main(sys.argv[1] if len(sys.argv) > 1 else "demo")
