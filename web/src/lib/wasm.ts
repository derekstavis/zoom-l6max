// `src/wasm/generated/` is built from `tools/firmware-patch-wasm` by
// `npm run wasm:build` (also run automatically before `dev`/`build`); it is
// not committed. See web/README.md.
import init, {
  applyPatches as applyPatchesWasm,
} from '../wasm/generated/firmware_patch_wasm.js'

let ready: Promise<void> | null = null

function ensureReady(): Promise<void> {
  ready ??= init().then(() => undefined)
  return ready
}

export interface AppliedPatch {
  name: string
  payload_address: string
  payload_length: number
}

export interface PatchReport {
  patched_sha256: string
  image_length: number
  patches: AppliedPatch[]
  update_sha256: string
  version: string
  usage: string
}

export interface PatchOutcome {
  package: Uint8Array
  report: PatchReport
}

/**
 * Applies `selected` patches (a non-empty subset of the three patch ids) to
 * `packageBytes` — a full Zoom update package — entirely in this browser
 * tab. Throws with a readable message on anything from an unrecognized
 * firmware to a corrupt upload.
 */
export async function applyPatches(
  packageBytes: Uint8Array,
  selected: string[],
): Promise<PatchOutcome> {
  await ensureReady()
  const result = applyPatchesWasm(packageBytes, selected)
  const report = JSON.parse(result.reportJson) as PatchReport
  return { package: result.package, report }
}
