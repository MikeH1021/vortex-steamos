#!/bin/sh
# Builds the Linux release archive: dist/vortex-steamos-<version>-linux-x86_64.tar.gz
#
# Run it on x86_64 Linux. It needs:
#   - Rust with the x86_64-unknown-linux-musl target (rustup target add ...)
#     and a musl C compiler (musl-tools on Debian and Ubuntu, musl on Arch)
#   - Godot 4.3 or newer with its Linux export templates installed
#
# Set GODOT to the editor binary when it is not called "godot".
# The GitHub workflow in .github/workflows/release.yml runs this for you.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

godot="${GODOT:-godot}"
target=x86_64-unknown-linux-musl
version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/vortex-cli/Cargo.toml | head -n 1)
name="vortex-steamos-$version-linux-x86_64"
stage="dist/stage"

rm -rf "$stage" "dist/$name" "dist/$name.tar.gz"
mkdir -p "$stage"

# A static binary runs the same on SteamOS (Arch) and Bazzite (Fedora),
# whatever C library version they ship.
cargo build --release --locked --target "$target" -p vortex-cli
cp "target/$target/release/vortex-cli" "$stage/vortex-cli"

"$godot" --headless --path ui --import
"$godot" --headless --path ui --export-release Linux "$root/$stage/vortex-steamos.x86_64"
if [ ! -s "$stage/vortex-steamos.x86_64" ]; then
    echo "Godot did not produce the app. Are the Linux export templates installed?" >&2
    exit 1
fi

cp packaging/vortex-steamos packaging/install.sh packaging/uninstall.sh "$stage/"
cp packaging/vortex-steamos.desktop.in "$stage/"
cp ui/icon.svg "$stage/vortex-steamos.svg"
cp LICENSE.md README.md "$stage/"
chmod 755 "$stage/vortex-steamos" "$stage/vortex-steamos.x86_64" "$stage/vortex-cli" "$stage/install.sh" "$stage/uninstall.sh"

mv "$stage" "dist/$name"
tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"
