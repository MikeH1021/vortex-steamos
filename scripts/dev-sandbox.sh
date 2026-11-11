#!/bin/sh
# Builds a pretend Steam library and a canned Nexus so the app can be run and
# clicked through on any machine, including macOS, without Steam, a game, or
# a network connection.
#
#   scripts/dev-sandbox.sh          create target/sandbox and print how to use it
#   eval "$(scripts/dev-sandbox.sh)"   create it and point this shell at it
#
# Then: cargo build -p vortex-cli && godot --path ui
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
box="$root/target/sandbox"
steam="$box/Steam"
nexus="$box/nexus"
rm -rf "$box"

manifest() { # app id, install dir
    printf '"AppState"\n{\n\t"appid"\t"%s"\n\t"installdir"\t"%s"\n\t"StateFlags"\t"4"\n}\n' "$1" "$2" \
        >"$steam/steamapps/appmanifest_$1.acf"
}
bethesda() { # app id, install dir, executable, master file, AppData folder
    game="$steam/steamapps/common/$2"
    mkdir -p "$game/Data/Textures" "$steam/steamapps/compatdata/$1/pfx/drive_c/users/steamuser/AppData/Local/$5"
    : >"$game/$3"
    : >"$game/Data/$4"
    manifest "$1" "$2"
}
plain() { # app id, install dir, program (may be in a subfolder)
    game="$steam/steamapps/common/$2"
    mkdir -p "$(dirname "$game/$3")"
    : >"$game/$3"
    manifest "$1" "$2"
}
mkdir -p "$steam/steamapps" "$nexus"
bethesda 489830 "Skyrim Special Edition" SkyrimSE.exe Skyrim.esm "Skyrim Special Edition"
bethesda 377160 "Fallout 4" Fallout4.exe Fallout4.esm Fallout4
plain 292030 "The Witcher 3" bin/x64/witcher3.exe
plain 1091500 "Cyberpunk 2077" bin/x64/Cyberpunk2077.exe
# Stardew Valley as Steam installs it on Linux: its own build, no Proton.
plain 413150 "Stardew Valley" StardewValley
echo '{}' >"$steam/steamapps/common/Stardew Valley/Stardew Valley.deps.json"
# A game imported from Vortex that keeps mods in Documents, inside its prefix.
plain 637090 "BATTLETECH" BattleTech.exe
mkdir -p "$steam/steamapps/compatdata/637090/pfx/drive_c/users/steamuser/Documents"

# What the pretend Nexus hands out for every download.
payload="$box/payload"
mkdir -p "$payload/textures/rocks"
echo plugin >"$payload/Campfire.esp"
echo texture >"$payload/textures/rocks/rock.dds"
(cd "$payload" && zip -qr "$nexus/payload.bin" .)
echo 2500 >"$nexus/payload.delay_ms"

cat >"$nexus/graphql.json" <<'JSON'
{"data":{"mods":{"nodes":[
{"modId":667,"name":"Campfire - Complete Camping System","summary":"Build campfires, set up tents and survive the cold.","author":"Chesko","version":"1.12","endorsements":210000,"downloads":5400000,"adultContent":false},
{"modId":12604,"name":"SkyUI","summary":"Elegant, PC-friendly interface mod with many advanced features.","author":"SkyUI Team","version":"5.2","endorsements":900000,"downloads":32000000,"adultContent":false}
]}}}
JSON
cat >"$nexus/files.json" <<'JSON'
{"files":[
{"file_id":1234,"name":"Campfire Main","file_name":"Campfire 1.12-667-1-12-1576028570.zip","version":"1.12","category_name":"MAIN","size_kb":4},
{"file_id":1300,"name":"Campfire Old","file_name":"Campfire-667-1-11.zip","version":"1.11","category_name":"OLD_VERSION","size_kb":90000}
]}
JSON
echo '[{"URI":"https://cdn.example/campfire.zip"}]' >"$nexus/download_link.json"
echo '{"name":"Campfire - Complete Camping System","version":"1.12"}' >"$nexus/mod.json"
# Change is_premium to false to try the free-account flow.
echo '{"user_id":1,"name":"Sandbox","is_premium":true}' >"$nexus/validate.json"

echo "export VORTEX_STEAM_ROOT='$steam'"
echo "export VORTEX_NEXUS_FIXTURE='$nexus'"
echo "export VORTEX_NEXUS_KEY=sandboxkey123"
echo "export XDG_STATE_HOME='$box/state'"
echo "# A pretend Mod Manager Download link, to pass to the app or the CLI:"
echo "#   godot --path ui -- 'nxm://skyrimspecialedition/mods/667/files/1234?key=abc&expires=1'"
