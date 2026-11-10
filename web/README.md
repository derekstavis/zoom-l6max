# L6 / L6max firmware patcher (web)

A static site that detects an uploaded L6 or L6max firmware package and applies
compatible firmware patches from [`../patches`](../patches) entirely client-side, via
WebAssembly. No server, no upload — see [`../tools/firmware-patch-wasm`](../tools/firmware-patch-wasm)
for how that's possible without ever shipping any of Zoom's firmware.

## Development

```sh
npm install
npm run dev
```

`predev`/`prebuild` run `npm run wasm:build` automatically, which compiles
[`firmware-patch-wasm`](../tools/firmware-patch-wasm) to `wasm32-unknown-unknown`
and runs `wasm-bindgen` to generate `src/wasm/generated/` — not committed,
since it's a build artifact of already-committed Rust source, not source
itself. Requires the `wasm32-unknown-unknown` Rust target and a `wasm-bindgen`
CLI matching the `wasm-bindgen` crate version in `../Cargo.lock`:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <version from Cargo.lock> --locked
```

## Build

```sh
npm run build
```

Output goes to `dist/`, rooted at `/` (see `vite.config.ts`) for GitHub
Pages, served from the custom domain `l6-patches.stavis.me`.

## Loading patches from an SD card

The site also distributes individual version-8 dynamic modules under
`public/loadable-patches/`. These are patch code only; no Zoom firmware image
is included. `manifest.json` records each module's model, format version,
loader ABI, and SHA-256. Current modules are MIDI Sync for L6 and L6max, and
Knob Dialog for L6max. The download name starts with `01-` so it has an
explicit position in the loader's filename ordering.

Choose **Temporary** as the installation method for each patch you want on the
SD card. The site creates one ZIP containing the model-named firmware update
and selected modules inside a `PATCHES/` folder. If any temporary patches are
selected, the update also installs Patch loader, a persistent component that
checks patch files for compatibility and integrity before loading them into
RAM. Extract the ZIP to the SD card's main folder and follow the model-specific
update instructions. The modules clear from RAM when the mixer restarts.
Patches without an SD module can only be included as built-in firmware patches.

Regenerate the hosted modules from local unpacked stock firmware for both
models:

```sh
tools/build-web-loadable-patches.sh /path/to/unpacked-l6max /path/to/unpacked-l6
```

The script writes only the generated `.BIN` modules and their manifest to
`public/loadable-patches/`. It uses the native `l6patch` builder to validate
the model image and produce the same files used by Patch loader.
The firmware templates used by the browser loader are regenerated separately:

```sh
cargo run --offline --locked -p firmware-patch-wasm-prebuild -- \
  /path/to/unpacked-l6max /path/to/unpacked-l6
```

That maintainer-only prebuild verifies both stock firmware revisions, strips
all copied firmware byte windows from the templates, and writes the templates
under `../tools/firmware-patch-wasm/templates/`.

## Stack

Vite + React + TypeScript, Tailwind CSS v4, [shadcn/ui](https://ui.shadcn.com)
(`radix-nova` preset).
