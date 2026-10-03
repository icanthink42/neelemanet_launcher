#!/usr/bin/env bash
set -euo pipefail

: "${BUILD_TARGET:?}" "${ARTIFACT:?}" "${RELEASE_TAG:?}" "${RELEASE_VERSION:?}"
mkdir -p dist/downloads
output="$PWD/dist/downloads/$ARTIFACT-$RELEASE_TAG"
binary="target/$BUILD_TARGET/release/neelemanet_launcher"

if [[ "$BUILD_TARGET" == *apple* ]]; then
  app='dist/dmg/NeelemaNet.app'
  mkdir -p "$app/Contents/MacOS"
  cp "$binary" "$app/Contents/MacOS/NeelemaNet"
  cp packaging/Info.plist "$app/Contents/Info.plist"
  python - <<'PY'
import os, pathlib, plistlib
path = pathlib.Path('dist/dmg/NeelemaNet.app/Contents/Info.plist')
with path.open('rb') as file:
    info = plistlib.load(file)
info['CFBundleShortVersionString'] = os.environ['RELEASE_VERSION'].split('-')[0].split('+')[0]
info['CFBundleVersion'] = os.environ['GITHUB_RUN_NUMBER']
with path.open('wb') as file:
    plistlib.dump(info, file)
PY
  codesign --force --deep --sign - "$app"
  codesign --verify --deep --strict "$app"
  ln -s /Applications dist/dmg/Applications
  hdiutil create -volname NeelemaNet -srcfolder dist/dmg -ov -format UDZO "$output.dmg"
  hdiutil verify "$output.dmg"
elif [[ "$BUILD_TARGET" == *windows* ]]; then
  cp "$binary.exe" "$output.exe"
else
  # Pin the packaging tool independently of the modpack catalog.
  deploy=dist/linuxdeploy-x86_64.AppImage
  curl --fail --location --retry 3 \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage \
    --output "$deploy"
  echo "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d  $deploy" | sha256sum --check
  chmod +x "$deploy"
  # Always start fresh: otherwise reruns can retain incompatible system libraries.
  rm -rf dist/AppDir
  mkdir -p dist/AppDir/usr/bin
  cp "$binary" dist/AppDir/usr/bin/NeelemaNet
  # Use the desktop's graphics/input stack. An older bundled Wayland library can
  # stop the host Mesa driver loading; old xkbcommon cannot parse newer Compose
  # files. Do not override linuxdeploy's system-library exclusions with --library.
  exclusions=()
  for name in 'libwayland-*.so*' 'libxkbcommon*.so*' 'libX11*.so*' 'libxcb*.so*' 'libEGL*.so*' 'libGL*.so*'; do
    exclusions+=(--exclude-library "$name")
  done
  # Optional X11 helpers are not installed on every desktop. Bundle these, while
  # keeping the core X11/XCB/xkbcommon libraries above supplied by the system.
  # In particular xkbcommon-x11 must match the system xkbcommon, not the build OS.
  libraries=()
  for name in libXcursor.so.1 libXi.so.6 libXrandr.so.2; do
    library=$(ldconfig -p | awk -v name="$name" '$1 == name && /x86-64/ && !found {print $NF; found=1}')
    test -n "$library" || { echo "Missing build library: $name" >&2; exit 1; }
    libraries+=(--library "$library")
  done
  # Extract-and-run lets packaging and its smoke test work on CI without FUSE.
  export APPIMAGE_EXTRACT_AND_RUN=1
  # Cargo's release profile already strips the binary; keep system libraries intact.
  export NO_STRIP=1
  export LDAI_OUTPUT="$output.AppImage" LINUXDEPLOY_OUTPUT_VERSION="$RELEASE_VERSION"
  "$deploy" --appdir dist/AppDir --executable dist/AppDir/usr/bin/NeelemaNet \
    --desktop-file packaging/net.neelemanet.launcher.desktop \
    --icon-file packaging/neelemanet.svg "${exclusions[@]}" "${libraries[@]}" --output appimage
  chmod +x "$output.AppImage"
  "$output.AppImage" --version
fi
