#!/usr/bin/env bash
# Run after linux-package on the same runner so sources match bundled libraries.
# Requires deb-src entries to be enabled (see the Linux job).
set -euo pipefail

root=$(git rev-parse --show-toplevel)
out="${1:-$root/emulator/dist}"
stage=$(mktemp -d "${TMPDIR:-/tmp/}l6-release-sources.XXXXXX")
trap 'rm -rf "$stage"' EXIT
sources="$stage/L6max-Emulator-linux-sources"
mkdir -p "$sources/project" "$sources/native" "$sources/rust/.cargo" "$sources/appimage"

# git archive excludes all ignored firmware, media, analysis databases and state.
git archive HEAD | tar -xf - -C "$sources/project"
# Include the exact patched QEMU tree and any dependencies fetched by configure.
tar -czf "$sources/qemu-11.1.2.tar.gz" \
  --exclude=.git --exclude=build --exclude=.venv \
  -C "$root/emulator/qemu/build-source" .
cargo vendor --locked "$sources/rust/vendor" > "$sources/rust/.cargo/config.toml"
# cargo vendor emits an absolute path; make the archive relocatable.
sed -i "s|$sources/rust/vendor|vendor|g" "$sources/rust/.cargo/config.toml"
cp Cargo.lock rust-toolchain.toml "$sources/rust/"

resources="$out/L6max-Emulator.AppDir/usr/share/l6max-emulator"
cp "$resources/native-dependencies.json" "$sources/native/dependencies.json"
cp "$resources/rust-dependencies.json" "$sources/rust/"
# Refuse to publish a library without the Debian source it was built from.
jq -e 'all(.[]; .package != null)' "$resources/native-dependencies.json" >/dev/null
# Source packages are looked up by binary package name and installed version;
# apt-get resolves the matching source package and its .dsc, tarballs and patches.
while IFS= read -r package; do
  name=${package% *}
  version=${package##* }
  mkdir -p "$sources/native/$name"
  (cd "$sources/native/$name" && apt-get source --download-only "$name=$version")
  ls "$sources/native/$name"/*.dsc >/dev/null
done < <(jq -r '.[].package' "$resources/native-dependencies.json" | sort -u)

# The AppImage embeds the type 2 runtime and is assembled by appimagetool.
# Versions and checksums are the ones the workflow verified before packaging.
while IFS=' ' read -r name url; do
  curl -fsSL --retry 3 -o "$sources/appimage/$name" "$url"
done <<'LIST'
type2-runtime-20251108.tar.gz https://github.com/AppImage/type2-runtime/archive/refs/tags/20251108.tar.gz
appimagetool-1.9.1.tar.gz https://github.com/AppImage/appimagetool/archive/refs/tags/1.9.1.tar.gz
LIST

{
  git rev-parse HEAD
  git -C emulator/qemu/upstream rev-parse HEAD
  rustc --version
  cargo --version
  lsb_release -ds
  gcc --version | head -1
} > "$sources/build-versions.txt"
cat > "$sources/README.txt" <<'EOF'
Sources matching the companion Linux AppImage:
  project/ contains this project's tagged tracked files, including model patches.
  qemu-11.1.2.tar.gz contains QEMU plus the model installed by qemu-build.
  rust/vendor/ contains the locked Cargo dependencies and their notices.
  native/ contains the Debian source packages (.dsc, upstream tarballs, patches)
          for every dynamic library bundled in the AppImage, at the installed version.
  appimage/ contains the sources of the AppImage runtime and appimagetool used.

Build instructions and workflow are under project/docs/guides/linux.md and
project/.github/workflows/. Unpack QEMU into project/emulator/qemu/upstream;
copy rust/vendor and rust/.cargo into project to use the vendored Rust sources.
Build the native libraries with `dpkg-source -x` and `dpkg-buildpackage` from the
supplied packages, then run qemu-build and linux-package.
The C library, graphics, audio and display-server libraries are provided by the
host system and are not redistributed. No manufacturer firmware is included.
EOF
tar -czf "$out/L6max-Emulator-linux-sources.tar.gz" -C "$stage" L6max-Emulator-linux-sources
