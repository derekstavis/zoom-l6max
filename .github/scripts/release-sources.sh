#!/usr/bin/env bash
# Run after packaging on the same machine so sources match bundled libraries.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
out="${1:-$root/emulator/dist}"
stage=$(mktemp -d "${TMPDIR:-/tmp/}l6-release-sources.XXXXXX")
tap=""
cleanup() {
  if [[ -n "$tap" ]]; then brew untap "$tap" >/dev/null 2>&1 || true; fi
  rm -rf "$stage"
}
trap cleanup EXIT
sources="$stage/L6max-Emulator-sources"
mkdir -p "$sources/project" "$sources/native" "$sources/rust/.cargo"

# git archive excludes all ignored firmware, media, analysis databases and state.
git archive HEAD | tar -xf - -C "$sources/project"
# Include the exact patched QEMU tree and any dependencies fetched by configure.
tar -czf "$sources/qemu-11.1.2.tar.gz" \
  --exclude=.git --exclude=build --exclude=.venv \
  -C "$root/emulator/qemu/build-source" .
cargo vendor --locked "$sources/rust/vendor" > "$sources/rust/.cargo/config.toml"
# cargo vendor emits an absolute path; make the archive relocatable.
sed -i '' "s|$sources/rust/vendor|vendor|g" "$sources/rust/.cargo/config.toml"
cp Cargo.lock rust-toolchain.toml "$sources/rust/"

manifest="$out/L6max Emulator.app/Contents/Resources/native-dependencies.json"
cp "$manifest" "$sources/native/dependencies.json"
# Refuse to publish a library without a known matching Homebrew source recipe.
jq -e 'all(.[]; .package != null)' "$manifest" >/dev/null
export HOMEBREW_CACHE="$stage/brew-cache"
export HOMEBREW_NO_AUTO_UPDATE=1
export HOMEBREW_DEVELOPER=1
# A dead primary mirror must time out so Homebrew can try its fallbacks.
printf '%s\n' 'connect-timeout = 15' 'max-time = 180' > "$stage/curlrc"
export HOMEBREW_CURLRC="$stage/curlrc"
export HOMEBREW_CURL_RETRIES=1
# Current Homebrew accepts copied recipes only inside a registered tap.
# This temporary tap installs no packages and is removed when collection ends.
tap="l6max/release-sources-$$"
brew tap-new --no-git "$tap"
tap_root=$(brew --repo "$tap")
while IFS= read -r package; do
  formula=${package% *}
  version=${package##* }
  cellar=$(brew --cellar "$formula")
  recipe="$cellar/$version/.brew/$formula.rb"
  test -f "$recipe"
  destination="$sources/native/$formula-$version"
  mkdir -p "$destination"
  cp "$recipe" "$destination/"
  cp "$cellar/$version/INSTALL_RECEIPT.json" "$destination/"
  # Keep the actual build recipe above. For fetching, prefer GNU's kernel.org
  # mirror over ftpmirror.gnu.org, whose redirect lookup can hang. Checksums
  # and all other URLs remain those of the installed bottle's recipe.
  mkdir -p "$destination/fetch"
  sed 's|https://ftpmirror.gnu.org/gnu/|https://mirrors.kernel.org/gnu/|g' \
    "$recipe" > "$destination/fetch/$formula.rb"
  cp "$destination/fetch/$formula.rb" "$tap_root/Formula/$formula.rb"
  brew fetch --formula --build-from-source "$tap/$formula"
done < <(jq -r '.[].package' "$manifest" | sort -u)
if [[ -d "$HOMEBREW_CACHE/downloads" ]]; then
  cp -RL "$HOMEBREW_CACHE/downloads" "$sources/native/downloads"
fi
cp "$out/L6max Emulator.app/Contents/Resources/rust-dependencies.json" "$sources/rust/"
{
  git rev-parse HEAD
  git -C emulator/qemu/upstream rev-parse HEAD
  rustc --version
  cargo --version
  xcodebuild -version
  brew --version
} > "$sources/build-versions.txt"
cat > "$sources/README.txt" <<'EOF'
Sources matching the companion macOS application:
  project/ contains this project's tagged tracked files, including model patches.
  qemu-11.1.2.tar.gz contains QEMU plus the model installed by qemu-build.
  rust/vendor/ contains the locked Cargo dependencies and their notices.
  native/ contains installed Homebrew recipes, receipts and downloaded sources,
          resources and patches for every bundled non-system dynamic library.

Build instructions and workflow are under project/docs/guides/macos-app.md and
project/.github/workflows/. Unpack QEMU into project/emulator/qemu/upstream;
copy rust/vendor and rust/.cargo into project to use the vendored Rust sources.
Build native dependencies from the supplied recipes/source archives (brew fetch
verifies their embedded checksums), then run qemu-build and macos-package.
Apple's SDK, system libraries and developer tools are not redistributed.
No manufacturer firmware is included.
EOF
tar -czf "$out/L6max-Emulator-sources.tar.gz" -C "$stage" L6max-Emulator-sources
