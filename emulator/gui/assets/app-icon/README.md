# Application icon

Original project artwork, covered by the root MIT license. `icon.svg` is the
editable source; `L6max.icns` contains macOS sizes from 16 to 1024 pixels.
It depicts a display and three indicator-ring knobs without manufacturer logos.

Regenerate with `rsvg-convert` and macOS `iconutil`:

```sh
mkdir -p /tmp/l6-app.iconset
for size in 16 32 128 256 512; do
  rsvg-convert -w "$size" -h "$size" emulator/gui/assets/app-icon/icon.svg -o "/tmp/l6-app.iconset/icon_${size}x${size}.png"
  doubled=$((size * 2))
  rsvg-convert -w "$doubled" -h "$doubled" emulator/gui/assets/app-icon/icon.svg -o "/tmp/l6-app.iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns /tmp/l6-app.iconset -o emulator/gui/assets/app-icon/L6max.icns
```
