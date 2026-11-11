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
}

build_module L6max "$l6max_source" midi-sync MIDI.BIN
build_module L6 "$l6_source" midi-sync MIDI.BIN
build_module L6max "$l6max_source" knob-dialog KNOB.BIN

python3 - "$repo_root/web/public/loadable-patches" <<'PY'
import hashlib
import json
import pathlib
import struct
import sys

root = pathlib.Path(sys.argv[1])
modules = []
for patch_id, model, file in [
    ("midi-sync", "L6max", "L6max/MIDI.BIN"),
    ("midi-sync", "L6", "L6/MIDI.BIN"),
    ("knob-dialog", "L6max", "L6max/KNOB.BIN"),
]:
    data = (root / file).read_bytes()
    if data[:4] != b"L6SB":
        sys.exit(f"{file}: not an SD patch module")
    # Record what the module declares rather than restating the format here.
    format_version, loader_abi = struct.unpack_from("<HH", data, 4)
    modules.append({
        "id": patch_id,
        "model": model,
        "file": file,
        "format_version": format_version,
        "loader_abi": loader_abi,
        "sha256": hashlib.sha256(data).hexdigest(),
    })
with open(root / "manifest.json", "w", encoding="utf-8") as manifest:
    json.dump({"modules": modules}, manifest, indent=2)
    manifest.write("\n")
PY

printf 'Updated web loadable patch assets.\n'
