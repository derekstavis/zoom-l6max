# CI and releases

[Documentation index](../README.md)

## Checks

`.github/workflows/ci.yml` runs for pull requests, pushes to `main`, and manual
dispatch. It also provides the reusable checks used by the release workflow.
The Apple silicon `macos-15` runner uses Xcode 26.3 and the repository's pinned
Rust toolchain. Official GitHub actions are pinned to commit IDs.

- Rust formatting, all-target workspace checks, and workspace unit tests.
- The standalone C GPIO indicator decoder test.
- Swift `L6Kit` tests using simulated devices.
- A build of QEMU 11.1.2 with the custom board and minimal optional dependencies.
- App assembly, recursive dependency relocation, signature verification, and
  execution of the relocated QEMU machine-list command.
- A downloadable debug app ZIP retained as a workflow artifact for 14 days.

The workflows use no manufacturer firmware. Guest execution, native controls,
and firmware update/patch diagnostics still require local firmware and remain
manual checks described in [testing](testing.md).

QEMU is initialized as a non-recursive submodule. Its unrelated ROM submodules
are not needed for this board. The builder stages a copy before applying model
sources; the submodule stays at its pinned commit. QEMU's configure step fetches
its own pinned build dependencies. Cargo dependencies are cached; builds are
locked to `Cargo.lock`.

## Publish a macOS release

After CI succeeds on the desired commit, create and push an emulator version tag:

```sh
git tag -a v0.1.0 -m 'L6max Emulator 0.1.0'
git push origin v0.1.0
```

`.github/workflows/release.yml` responds to `v*` tags. Use
`vMAJOR.MINOR.PATCH` or `vMAJOR.MINOR.PATCH-suffix`; suffixed tags publish
prereleases. These are app versions, separate from firmware markers.

The release calls the same CI checks, builds an optimized app, then publishes:

| Asset | Contents |
| --- | --- |
| `L6max-Emulator-macos-arm64.zip` | Firmware-free, ad-hoc signed app for Apple silicon |
| `L6max-Emulator-sources.tar.gz` | Tagged project, patched QEMU, locked Rust dependencies, matching native dependency sources and build recipes |
| `SHA256SUMS` | SHA-256 hashes of both archives |

The tag supplies the app version. The bundle's minimum OS is calculated from
its actual Mach-O deployment targets and included in the release notes.
Source collection uses the installed Homebrew recipes to fetch matching source
archives, resources, and patches, rather than potentially newer upstream sources.
Unknown library origins or missing sources fail the release before publication.

Only the publication job has `contents: write`, through `GITHUB_TOKEN`; no
additional GitHub token is needed. A draft is published after all assets upload.
Re-running a failed upload can complete the draft, while an already published
release is not replaced. GitHub Actions must be enabled in the repository.

- The app is not Developer ID signed or notarized; no Apple signing secrets are
  configured. The release notes explain Gatekeeper behavior.
- Only Apple silicon builds are produced. Intel and universal bundles are not
  verified.
- The hosted runner's OS differs from local development. A successful package
  check verifies construction and loader relocation, not full guest/UI behavior.
