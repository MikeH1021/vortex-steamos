#!/bin/sh
# Installs Vortex for SteamOS into your home folder. Nothing needs root, and
# nothing touches the read-only system image, so it survives SteamOS updates.
#
#   ./install.sh             install or update, and add it to your Steam library
#   ./install.sh --no-steam  do not add it to Steam
#   ./install.sh --no-links  do not take over nxm:// download links
#
# Once it is in your Steam library it shows up under Non-Steam in Game Mode
# and in Big Picture, and Steam hands it your controller.
set -eu

add_to_steam=1
take_links=1
for arg in "$@"; do
    case "$arg" in
        --add-to-steam) add_to_steam=1 ;;
        --no-steam) add_to_steam=0 ;;
        --no-links) take_links=0 ;;
        -h | --help)
            sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            exit 2
            ;;
    esac
done

src=$(cd "$(dirname "$0")" && pwd)
data="${XDG_DATA_HOME:-$HOME/.local/share}"
app="$data/vortex-steamos"
entry="$data/applications/vortex-steamos.desktop"

for file in vortex-steamos vortex-steamos.x86_64 vortex-cli vortex-steamos.svg vortex-steamos.desktop.in; do
    if [ ! -f "$src/$file" ]; then
        echo "$file is missing next to install.sh. Extract the whole archive first." >&2
        exit 1
    fi
done

mkdir -p "$app" "$data/applications" "$data/icons/hicolor/scalable/apps" "$HOME/.local/bin"
install -m 755 "$src/vortex-steamos" "$src/vortex-steamos.x86_64" "$src/vortex-cli" "$app/"
install -m 755 "$src/uninstall.sh" "$app/"
install -m 644 "$src/vortex-steamos.svg" "$data/icons/hicolor/scalable/apps/vortex-steamos.svg"
sed "s|@APP@|$app|g" "$src/vortex-steamos.desktop.in" >"$entry"
chmod 644 "$entry"
ln -sf "$app/vortex-steamos" "$HOME/.local/bin/vortex-steamos"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data/applications" >/dev/null 2>&1 || true
fi

echo "Installed to $app"

if [ "$take_links" -eq 1 ]; then
    if command -v xdg-mime >/dev/null 2>&1 && xdg-mime default vortex-steamos.desktop x-scheme-handler/nxm; then
        echo "Mod Manager Download links on nexusmods.com now open in Vortex for SteamOS."
    else
        echo "Could not register the nxm:// link handler. Open the app, go to Downloads, and choose Catch Nexus download links." >&2
    fi
fi

if [ "$add_to_steam" -eq 1 ]; then
    # Adding the same entry twice gives two tiles, so it is done once.
    marker="$app/.added-to-steam"
    if [ -f "$marker" ]; then
        echo "Already in your Steam library, under Non-Steam."
    elif command -v steamos-add-to-steam >/dev/null 2>&1; then
        if steamos-add-to-steam "$entry"; then
            : >"$marker"
            echo "Added to Steam. Look under Non-Steam in Game Mode or Big Picture."
        else
            echo "Steam did not take the shortcut. In Steam, use Games > Add a Non-Steam Game and pick Vortex for SteamOS." >&2
        fi
    else
        echo "To launch it from Game Mode or Big Picture: in Steam, use Games > Add a Non-Steam Game and pick Vortex for SteamOS."
    fi
fi
