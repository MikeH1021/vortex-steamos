#!/usr/bin/env python3
"""Applies, checks, or takes back out the local fixes for Multi Companion Mod
Remastered and MCME Autonomous Romance in The Witcher 3. See README.md here.

    apply.py                  apply the fixes to the mods as they are staged
    apply.py --check          say what state each script is in, change nothing
    apply.py --revert         take the fixes back out
    apply.py --staging DIR    use another staging folder than the default
                              (~/.local/share/Steam/steamapps/vortex/witcher3)

The scripts are edited where the mod manager staged them. The game folder
holds hardlinks to those same files, so nothing has to be deployed again.
The files are rewritten in place for that reason: replacing them would leave
the game with the old copy.
"""
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
PATCH = os.path.join(HERE, "mcmr-local-fixes.patch")
MANIFEST = os.path.join(HERE, "manifest.tsv")
DEFAULT_STAGING = os.path.expanduser("~/.local/share/Steam/steamapps/vortex/witcher3")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def game_running():
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            with open("/proc/%s/cmdline" % pid, "rb") as handle:
                first = handle.read().split(b"\0")[0].lower().replace(b"\\", b"/")
        except OSError:
            continue
        if first.endswith(b"/witcher3.exe"):
            return True
    return False


def find_scripts(staging, mod_folder):
    """The content/scripts folder of a mod, whatever id the manager gave it."""
    mods = os.path.join(staging, "mods")
    if not os.path.isdir(mods):
        return None
    for mod_id in sorted(os.listdir(mods)):
        candidate = os.path.join(mods, mod_id, "mods", mod_folder, "content", "scripts")
        if os.path.isdir(candidate):
            return candidate
    return None


def main():
    args = sys.argv[1:]
    staging = DEFAULT_STAGING
    if "--staging" in args:
        at = args.index("--staging")
        staging = args[at + 1]
        del args[at:at + 2]
    check = "--check" in args
    revert = "--revert" in args
    if [arg for arg in args if arg not in ("--check", "--revert")] or (check and revert):
        sys.stderr.write(__doc__)
        return 2

    rows = []
    with open(MANIFEST) as handle:
        for line in handle:
            if line.strip():
                rows.append(line.rstrip("\n").split("\t"))

    files = []
    states = set()
    for mod_folder, relative, original_sha, patched_sha in rows:
        scripts = find_scripts(staging, mod_folder)
        path = os.path.join(scripts, relative) if scripts else None
        if not path or not os.path.isfile(path):
            print("missing    %s/%s  (is the mod installed?)" % (mod_folder, relative))
            states.add("missing")
            continue
        with open(path, "rb") as handle:
            data = handle.read()
        digest = sha(data)
        state = "original" if digest == original_sha else "patched" if digest == patched_sha else "different"
        states.add(state)
        print("%-10s %s/%s" % (state, mod_folder, relative))
        files.append((mod_folder, relative, path, data, state))

    if check:
        if "different" in states:
            print("\nA 'different' script is neither the version these fixes were made for nor the fixed one.")
            print("The mod was probably updated. The patch may still apply; try it and read the result.")
        return 0 if states <= {"patched"} or states <= {"original"} else 1
    if "missing" in states:
        print("\nNothing was changed.")
        return 1
    wanted_done = "original" if revert else "patched"
    if states == {wanted_done}:
        print("\nAlready %s. Nothing to do." % ("without the fixes" if revert else "fixed"))
        return 0
    if game_running():
        print("\nThe game is running. Close it first: scripts are compiled when it starts.")
        return 1

    # The patch is against files with Unix line endings. The mods ship a mix,
    # so each file is normalised for patching and given its own endings back.
    work = tempfile.mkdtemp(prefix="mcmr-patch-")
    try:
        todo = []
        for mod_folder, relative, path, data, state in files:
            if state == wanted_done:
                continue
            crlf = b"\r\n" in data
            target = os.path.join(work, mod_folder, relative)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "wb") as handle:
                handle.write(data.replace(b"\r\n", b"\n"))
            todo.append((mod_folder, relative, path, data, crlf, target))
        include = []
        for mod_folder, relative, *_ in todo:
            include += ["--include", "%s/%s" % (mod_folder, relative)]
        command = ["git", "apply", "-p1"] + (["-R"] if revert else []) + include + [PATCH]
        result = subprocess.run(command, cwd=work, capture_output=True, text=True)
        if result.returncode != 0:
            print("\nThe patch does not fit these scripts, so nothing was changed:")
            print(result.stderr.strip())
            return 1
        backups = os.path.join(staging, "patch-backups")
        os.makedirs(backups, exist_ok=True)
        for mod_folder, relative, path, data, crlf, target in todo:
            if not revert:
                backup = os.path.join(backups, os.path.basename(relative) + ".orig")
                if not os.path.exists(backup):
                    with open(backup, "wb") as handle:
                        handle.write(data)
            with open(target, "rb") as handle:
                new = handle.read()
            if crlf:
                new = new.replace(b"\n", b"\r\n")
            with open(path, "r+b") as handle:  # in place: the game folder links to this file
                handle.seek(0)
                handle.truncate()
                handle.write(new)
            print("%-10s %s/%s" % ("reverted" if revert else "fixed", mod_folder, relative))
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print("\nDone. Start the game; it compiles the scripts again on launch.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
