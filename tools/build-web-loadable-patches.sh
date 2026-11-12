#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: tools/build-web-loadable-patches.sh L6MAX_UNPACKED_DIR L6_UNPACKED_DIR

Builds the versioned SD modules declared for web or emulator delivery in each
patches/<name>/package.toml. Inputs must be unpacked stock firmware directories
accepted by l6patch. Firmware is read locally and is never copied into web/public.
USAGE
  exit 2
}

[[ $# -eq 2 ]] || usage
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
l6max_source="$1"
l6_source="$2"
temp_root="$(mktemp -d)"
trap 'rm -rf "$temp_root"' EXIT
catalog="$temp_root/catalog.json"
records="$temp_root/modules.tsv"

cargo run --offline --locked --manifest-path "$repo_root/Cargo.toml" \
  -p firmware-patch-core --bin patch-catalog > "$catalog"
python3 - "$catalog" > "$records" <<'PY_MAP'
import json
import sys

catalog = json.load(open(sys.argv[1], encoding="utf-8"))
for package in catalog["packages"]:
    models = set(package["website_sd_models"]) | set(package["emulator_sd_models"])
    for model in sorted(models):
        print(package["name"], model, package["file_name"], sep="\t")
PY_MAP

module_root="$repo_root/web/public/loadable-patches"
mkdir -p "$module_root"
: > "$temp_root/built.tsv"
while IFS=$'\t' read -r patch model file_name; do
  [[ -n "$patch" ]] || continue
  source="$l6max_source"
  [[ "$model" == "L6" ]] && source="$l6_source"
  output="$temp_root/$model-$patch"
  cargo run --offline --locked --manifest-path "$repo_root/Cargo.toml" -p l6patch -- \
    "$source" "$output" --experimental-patch-bundle --patch "$patch"
  module_file="$(python3 - "$output/patch.json" <<'PY_REPORT'
import json
import pathlib
import sys
report = json.load(open(sys.argv[1], encoding="utf-8"))
print(pathlib.Path(report["patches"][0]["file"]).name)
PY_REPORT
)"
  [[ "$module_file" == "$file_name" ]] || {
    echo "$patch $model: package catalog expects $file_name but l6patch emitted $module_file" >&2
    exit 1
  }
  destination="$module_root/$model"
  mkdir -p "$destination"
  cp "$output/PATCHES/$module_file" "$destination/$module_file"
  printf '%s\t%s\t%s\n' "$patch" "$model" "$module_file" >> "$temp_root/built.tsv"
done < "$records"

python3 - "$module_root" "$temp_root/built.tsv" <<'PY_MANIFEST'
import hashlib
import json
import pathlib
import struct
import sys

root = pathlib.Path(sys.argv[1])
modules = []
for line in pathlib.Path(sys.argv[2]).read_text(encoding="utf-8").splitlines():
    patch, model, file_name = line.split("\t")
    relative = f"{model}/{file_name}"
    data = (root / relative).read_bytes()
    if data[:4] != b"L6SB":
        sys.exit(f"{relative}: not an SD patch module")
    format_version, loader_abi = struct.unpack_from("<HH", data, 4)
    modules.append({
        "id": patch,
        "model": model,
        "file": relative,
        "format_version": format_version,
        "loader_abi": loader_abi,
        "sha256": hashlib.sha256(data).hexdigest(),
    })
(root / "manifest.json").write_text(
    json.dumps({"modules": modules}, indent=2) + "\n", encoding="utf-8"
)
PY_MANIFEST

printf 'Updated versioned SD patch assets from package.toml.\n'
