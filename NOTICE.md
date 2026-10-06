# Licenses and external materials

Original Rust code, patch source, diagnostic tools, and project documentation are
licensed under [MIT](LICENSE), except where another license is stated.

## QEMU model

The C board/peripheral model and C tests under `emulator/qemu/` are
GPL-2.0-or-later. The license text is included in
[emulator/qemu/COPYING](emulator/qemu/COPYING). QEMU is built separately from its
upstream source, pinned by the submodule under `emulator/qemu/upstream/`.

## Application icon

The mixer-style application icon under `emulator/gui/assets/app-icon/` is
original project artwork, licensed under MIT.

## Public icons

- [Lucide icons](emulator/gui/assets/icons/README.md): ISC, with MIT notices for
  Feather-derived icons. Preserve the bundled
  [license](emulator/gui/assets/icons/LICENSE).
- [Phosphor icons](emulator/gui/assets/icons/phosphor/README.md): MIT. Preserve
  the bundled [license](emulator/gui/assets/icons/phosphor/LICENSE).

Cargo dependencies retain their own licenses; the project license does not
relicense them. The macOS packager includes Cargo dependency inventories and
available license/notice files, as well as native-library inventories and
Homebrew-installed license text. See the
[packaging guide](docs/guides/macos-app.md#distribution-status) for public binary
release requirements.

## Firmware and branding

Firmware packages, extracted images, ROM/font/bitmap dumps,
and vendor applications are local inputs or generated artifacts. They are not
included in the source distribution and are not covered by the project license.
The patch builder reads displaced instructions from locally supplied firmware;
the committed hook table contains addresses, lengths, and validation hashes.

No manufacturer logo, traced panel artwork, product photograph, or vendor manual
is bundled. The native panel is built from project components and licensed public
icons. Product names in compatibility documentation and functional control labels
identify the hardware being studied. This is an independent project and is not
affiliated with or endorsed by ZOOM.

The documentation screenshot in `docs/images/` is a capture of the project's
native UI running locally supplied firmware. It is not a manufacturer product
photograph; the project license does not grant rights to the firmware itself.

Provide your own firmware locally. Keep firmware, runtime display dumps, generated
patch packages, and vendor applications out of source commits and releases.
