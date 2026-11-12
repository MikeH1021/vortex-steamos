#!/usr/bin/env python3
"""Gives the app's tile in the Steam library its artwork and icon.

    steam-artwork.py <launcher> <art folder>            add the artwork
    steam-artwork.py --remove <launcher>                take it away again

Steam keeps non-Steam games in userdata/<account>/config/shortcuts.vdf and
looks for their pictures in the grid folder next to it, named after the
shortcut's id. The pictures can be added while Steam runs. The icon is a
field inside shortcuts.vdf, which Steam rewrites when it exits, so that is
only changed while Steam is closed.
"""
import os
import shutil
import struct
import subprocess
import sys
import time

ART = {  # file in the art folder -> name Steam looks for, after the id
    "portrait.png": "p.png",
    "header.png": ".png",
    "hero.png": "_hero.png",
    "logo.png": "_logo.png",
}


class Unsupported(Exception):
    pass


def parse(data, at=0):
    """Reads one object of Steam's binary key-value format, in file order."""
    items = []
    while True:
        kind = data[at]
        at += 1
        if kind == 8:
            return items, at
        end = data.index(b"\0", at)
        key = data[at:end]
        at = end + 1
        if kind == 0:
            value, at = parse(data, at)
        elif kind == 1:
            end = data.index(b"\0", at)
            value = data[at:end]
            at = end + 1
        elif kind == 2:
            value = struct.unpack_from("<I", data, at)[0]
            at += 4
        else:
            raise Unsupported(kind)
        items.append([kind, key, value])


def dump(items):
    out = bytearray()
    for kind, key, value in items:
        out.append(kind)
        out += key + b"\0"
        if kind == 0:
            out += dump(value)
        elif kind == 1:
            out += value + b"\0"
        else:
            out += struct.pack("<I", value)
    out.append(8)
    return bytes(out)


def field(items, name):
    for item in items:
        if item[1].lower() == name:
            return item
    return None


def shortcut_files():
    home = os.path.expanduser("~")
    roots = [
        os.path.join(os.environ.get("XDG_DATA_HOME", os.path.join(home, ".local/share")), "Steam"),
        os.path.join(home, ".steam/steam"),
        os.path.join(home, ".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    seen = set()
    for root in roots:
        userdata = os.path.join(root, "userdata")
        if not os.path.isdir(userdata):
            continue
        for account in sorted(os.listdir(userdata)):
            path = os.path.realpath(os.path.join(userdata, account, "config", "shortcuts.vdf"))
            if path not in seen and os.path.isfile(path):
                seen.add(path)
                yield path


def ours(path, launcher):
    """The shortcuts in one file that start this app: (whole file, entries)."""
    with open(path, "rb") as handle:
        data = handle.read()
    try:
        tree, _ = parse(data)
    except (Unsupported, ValueError, IndexError, struct.error):
        return None, None, []
    found = []
    shortcuts = field(tree, b"shortcuts")
    for _, _, entry in shortcuts[2] if shortcuts else []:
        exe = field(entry, b"exe")
        if exe is None:
            continue
        target = exe[2].decode("utf-8", "replace").strip().strip('"')
        if target and os.path.realpath(target) == launcher:
            found.append(entry)
    return data, tree, found


def steam_running():
    return subprocess.run(["pgrep", "-x", "steam"], stdout=subprocess.DEVNULL).returncode == 0


def main():
    args = sys.argv[1:]
    remove = bool(args) and args[0] == "--remove"
    if remove:
        args = args[1:]
    if len(args) != (1 if remove else 2):
        sys.stderr.write(__doc__)
        return 2
    launcher = os.path.realpath(args[0])
    art = None if remove else args[1]
    icon = None if remove else os.path.join(art, "icon.png")

    # Steam writes a shortcut it was just handed a moment later.
    done = 0
    for attempt in range(1 if remove else 10):
        for path in shortcut_files():
            data, tree, entries = ours(path, launcher)
            if not entries:
                continue
            grid = os.path.join(os.path.dirname(path), "grid")
            os.makedirs(grid, exist_ok=True)
            changed = False
            for entry in entries:
                appid = field(entry, b"appid")
                if appid is None or appid[0] != 2:
                    continue
                for source, suffix in ART.items():
                    target = os.path.join(grid, "%d%s" % (appid[2], suffix))
                    if remove:
                        if os.path.exists(target):
                            os.remove(target)
                    else:
                        shutil.copyfile(os.path.join(art, source), target)
                done += 1
                current = field(entry, b"icon")
                if icon and current is not None and current[0] == 1 and current[2] != os.fsencode(icon):
                    current[2] = os.fsencode(icon)
                    changed = True
            # Only rewrite a file we can reproduce byte for byte, and only
            # while Steam is not there to write its own copy over it.
            if changed and dump(parse(data)[0]) == data:
                if steam_running():
                    print("The tile gets its icon the next time this is run with Steam closed.")
                else:
                    shutil.copyfile(path, path + ".vortex_backup")
                    with open(path + ".tmp", "wb") as handle:
                        handle.write(dump(tree))
                    os.replace(path + ".tmp", path)
        if done or remove:
            break
        time.sleep(1)

    if not remove:
        if done:
            print("Added artwork to the Steam library tile. Restart Steam to see it.")
        else:
            print("Did not find this app in the Steam library, so no artwork was added.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
