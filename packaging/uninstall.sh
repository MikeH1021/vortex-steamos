#!/bin/sh
# Removes the app. Your mods, downloads, and Nexus sign-in are left alone.
#
# Mods stay linked into your games. To take them out first, run
#   vortex-cli purge --game <game>
# for each game before uninstalling. Staged mods live in
#   <steam library>/steamapps/vortex/
# and the saved Nexus key in ~/.config/vortex-steamos/.
set -eu

data="${XDG_DATA_HOME:-$HOME/.local/share}"
if command -v python3 >/dev/null 2>&1 && [ -f "$data/vortex-steamos/steam-artwork.py" ]; then
    python3 "$data/vortex-steamos/steam-artwork.py" --remove "$data/vortex-steamos/vortex-steamos" || true
fi
rm -f "$data/applications/vortex-steamos.desktop"
rm -f "$data/icons/hicolor/256x256/apps/vortex-steamos.png"
rm -f "$data/icons/hicolor/scalable/apps/vortex-steamos.svg"
rm -f "$HOME/.local/bin/vortex-steamos"
rm -rf "$data/vortex-steamos"
rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/vortex-steamos"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data/applications" >/dev/null 2>&1 || true
fi
echo "Vortex for SteamOS is removed. Mods and downloads are still in steamapps/vortex in each Steam library."
