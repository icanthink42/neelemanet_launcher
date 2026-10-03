"""Open the release AppImage on an isolated X11 or headless Wayland display."""

import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time


def stop(process):
    if process is not None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def main():
    appimage = Path(sys.argv[1]).resolve()
    backend = sys.argv[2]
    if backend not in ("x11", "wayland"):
        raise SystemExit("Expected x11 or wayland")
    appimage.chmod(appimage.stat().st_mode | 0o111)
    app = compositor = None
    with tempfile.TemporaryDirectory(prefix="neelemanet-gui-") as directory:
        root = Path(directory)
        env = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1", LIBGL_ALWAYS_SOFTWARE="1")
        env.pop("WAYLAND_SOCKET", None)
        env.pop("WAYLAND_DISPLAY", None)
        app_log = root / "app.log"
        compositor_log = root / "compositor.log"
        try:
            if backend == "wayland":
                env.pop("DISPLAY", None)
                env.update(XDG_RUNTIME_DIR=directory, WAYLAND_DISPLAY="smoke-wayland", WAYLAND_DEBUG="1")
                with compositor_log.open("w") as log:
                    compositor = subprocess.Popen(
                        ["weston", "--backend=headless-backend.so", "--use-pixman",
                         "--socket=smoke-wayland", "--idle-time=0", "--no-config"],
                        env=env, stdout=log, stderr=log, start_new_session=True,
                    )
                deadline = time.monotonic() + 15
                while not (root / "smoke-wayland").exists():
                    if compositor.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("Headless Wayland display failed to start")
                    time.sleep(0.1)
            with app_log.open("w") as log:
                # An absent local catalog isolates this check from network/Prism
                # installation while still exercising the actual GUI renderer.
                app = subprocess.Popen(
                    [str(appimage), "--data-dir", str(root / "data"),
                     "--catalog", str(root / "missing.toml")],
                    env=env, stdout=log, stderr=log, start_new_session=True,
                )
            deadline = time.monotonic() + 20
            while True:
                if app.poll() is not None:
                    raise RuntimeError(f"Launcher exited before opening its window: {app.returncode}")
                if backend == "wayland":
                    log = app_log.read_text()
                    mapped = re.search(r"wl_surface.*\.attach\(wl_buffer", log) is not None
                else:
                    windows = subprocess.check_output(["xwininfo", "-root", "-tree"], env=env, text=True)
                    mapped = '"NeelemaNet"' in windows
                if mapped:
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError("Launcher never displayed a window")
                time.sleep(0.25)
            try:
                app.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass
            else:
                raise RuntimeError(f"Launcher exited after opening its window: {app.returncode}")
            if "xkbcommon: ERROR:" in app_log.read_text():
                raise RuntimeError("Keyboard library failed to parse the desktop's layout/Compose data")
            print(f"AppImage opened and stayed running on {backend}.")
        except Exception:
            for log in (app_log, compositor_log):
                if log.exists():
                    print(log.read_text()[-12000:], file=sys.stderr)
            raise
        finally:
            stop(app)
            stop(compositor)


if __name__ == "__main__":
    main()
