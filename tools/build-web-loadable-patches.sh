#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: tools/build-web-loadable-patches.sh L6MAX_UNPACKED_DIR L6_UNPACKED_DIR

Builds the dynamic MIDI and knob-dialog modules hosted by the web patcher. Inputs must be
unpacked stock firmware directories accepted by l6patch. Firmware is read
locally and is never copied into web/public.
EOF
  exit 2
}

[[ $# -eq 2 ]] || usage
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
l6max_source="$1"
l6_source="$2"
temp_root="$(mktemp -d)"
trap 'rm -rf "$temp_root"' EXIT

build_module() {
  local model="$1"
  local source="$2"
  local patch="$3"
  local file_name="$4"
  local output="$temp_root/$model-$patch"
  local destination="$repo_root/web/public/loadable-patches/$model"

  cargo run --offline --locked --manifest-path "$repo_root/Cargo.toml" -p l6patch -- \
    "$source" "$output" --experimental-patch-bundle --patch "$patch" >&2
  mkdir -p "$destination"
  local module_file
  module_file="$(python3 - "$output/patch.json" <<'PY_REPORT'
import json
import pathlib
import sys
report = json.load(open(sys.argv[1], encoding="utf-8"))
print(pathlib.Path(report["patches"][0]["file"]).name)
PY_REPORT
)"
  cp "$output/PATCHES/$module_file" "$destination/$file_name"
  shasum -a 256 "$destination/$file_name" | awk '{print $1}'
}

l6max_midi_sha="$(build_module L6max "$l6max_source" midi-sync MIDI.BIN)"
l6_midi_sha="$(build_module L6 "$l6_source" midi-sync MIDI.BIN)"
l6max_knob_sha="$(build_module L6max "$l6max_source" knob-dialog KNOB.BIN)"

python3 - "$repo_root/web/public/loadable-patches/manifest.json" "$l6max_midi_sha" "$l6_midi_sha" "$l6max_knob_sha" <<'PY'
import json
import sys

manifest_path, l6max_midi_sha, l6_midi_sha, l6max_knob_sha = sys.argv[1:]
modules = [{
    "id": "midi-sync",
    "model": "L6max",
    "file": "L6max/MIDI.BIN",
    "format_version": 8,
    "loader_abi": 2,
    "sha256": l6max_midi_sha,
}]
modules.append({
    "id": "midi-sync",
    "model": "L6",
    "file": "L6/MIDI.BIN",
    "format_version": 8,
    "loader_abi": 2,
    "sha256": l6_midi_sha,
})
modules.append({
    "id": "knob-dialog",
    "model": "L6max",
    "file": "L6max/KNOB.BIN",
    "format_version": 8,
    "loader_abi": 2,
    "sha256": l6max_knob_sha,
})
with open(manifest_path, "w", encoding="utf-8") as manifest:
    json.dump({"modules": modules}, manifest, indent=2)
    manifest.write("\n")
PY

printf 'Updated web loadable patch assets.\n'
