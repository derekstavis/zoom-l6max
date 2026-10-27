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

## Stack

Vite + React + TypeScript, Tailwind CSS v4, [shadcn/ui](https://ui.shadcn.com)
(`radix-nova` preset).
