# Local fixes for Multi Companion Mod Remastered (The Witcher 3)

Script patches for two Nexus mods, worked out on one Bazzite machine on 2026-11-12 with the game running and its script log open. They are not part of the mod manager. They are kept here so they can be put back after either mod is reinstalled or updated.

| Mod | Nexus | Version patched |
| --- | --- | --- |
| Multi Companion Mod Remastered (MCMR) | witcher3 mod 13144, file 75050 | 3 |
| MCME - Autonomous Romance (Remastered) | witcher3 mod 14262, file 87172 | 2.3 |

Game: The Witcher 3: Wild Hunt Remastered 5.01 (Steam, through Proton).

The patches change the mods' own scripts. They belong to their authors; only the changed lines and a few lines around each are in this folder.

## What was wrong

With both mods installed correctly, a spawned companion walked while Geralt ran, never teleported to catch up, never spoke, and the romance add-on did nothing at all. The menu, spawning, and the `CallAllCompanions()` console command worked.

The script log showed two faults.

1. **Neither mod's start-up ran when a save loaded.** MCMR wraps `CR4Player.OnSpawned` and the add-on wraps `CR4Player.InitRemasterSettings` to add their repeating timers to the player. On this machine neither wrapper is called on load, so the timers are never added. Everything periodic hangs off them: follower speed, the catch-up teleport, banter, and all of the add-on. Why the wrappers are not called is not known. Three other installed mods wrap methods of the same class, which is one untested suspect.
2. **The follow behaviour was refused during loading.** `ForceAIBehavior` returns -1 when called while the save is still loading. MCMR stored that as if it were a running behaviour and never tried again, so the companion had no follow behaviour and drifted after Geralt at a walk.

## What the patch does

In MCMR (`modMCME_Remastered`):

- `local/MCME_RemasteredPlayerHooks.ws`: the body of the `OnSpawned` wrapper becomes a method, `mcm_PlayerInit`, that runs once. Before it starts the timers it puts every companion who is following right now on the saved follower list. Without that, the mod's world-loaded pass deletes them, because on an affected save the list was never written.
- `local/MCME_RemasteredNPCHooks.ws`: each companion gets a timer of its own, `mcm_selfTick`. It runs `mcm_PlayerInit` if that has not happened, and stands in for the mod's quick and slow updates whenever they have stopped arriving.
- `game/SpawnCompanionsSCMCC.ws`:
  - a refused follow behaviour (-1) is recorded as not started, and retried from the quick update;
  - a follower on foot who stays more than 12 m behind for several seconds has her follow behaviour restarted, which covers a cutscene dropping it. This is skipped while Geralt is on a horse, where she cannot keep up anyway and the catch-up teleport takes over;
  - a pause asked for through `StopFollowing(true)` is left alone, since the romance add-on uses that for idle poses and scenes and resumes on its own;
  - follower speed keeps pace with a running or sprinting Geralt, speeds up with distance, falls back to the direction Geralt faces when he has no velocity, and is capped at 4 like the mod's own horse code.
- `game/SpawnCompanions.ws`: counters and timestamps for the quick and slow updates.
- `game/mcm/MultiCompanionModExecutable.ws`: three console commands.

In the add-on (`modMCME_AutonomousRomance`):

- `local/romance/MCM_RomanceQoL.ws`: a companion's four-second update starts the add-on when it has not been started.

### Console commands added

- `mcmdiag`: shows update counts and each follower's state. It was not seen working during testing.
- `mcmfix`: restarts the timers and the follow behaviour.
- `mcmspeed`: switches a readout of the speed calculation on and off.

## Applying

Close the game first. Scripts are compiled when it starts.

```sh
patches/witcher3-mcmr/apply.py --check     # what state each script is in
patches/witcher3-mcmr/apply.py             # apply
patches/witcher3-mcmr/apply.py --revert    # take the fixes back out
```

It needs `python3` and `git`. It edits the scripts where the mod manager staged them, under `~/.local/share/Steam/steamapps/vortex/witcher3/mods/`. The game folder links to those same files, so nothing has to be deployed again. The first time, the untouched scripts are copied to `patch-backups/` in that staging folder.

`--check` names each script as `original`, `patched`, or `different`. `different` means the mod has changed since these fixes were written. The patch is then tried anyway and refuses as a whole if any part does not fit.

If the game shows a "Script Compilation Errors" window after applying, the mod has changed underneath the patch. Run `apply.py --revert`, or reinstall the mod, and press Retry Script Compilation.

## What was checked, and what was not

Checked in the running game, driven by scripted input, after a fresh load with no console commands:

- both mods start on their own, and the companion is kept;
- Triss runs and sprints beside Geralt;
- companion banter plays;
- the add-on starts a kiss scene by itself, speaks an afterglow line, raises affinity, and the companion follows again afterwards.

Checked in a second pass, with temporary test commands that are not part of the patch:

- **Catch-up teleport on foot.** Triss was moved 40 m and 60 m away. Each time the mod teleported her back within one of its four-second checks (at 36.8 m and 54.1 m).
- **Catch-up teleport while Geralt rides.** With Geralt at a canter and a gallop and Triss on foot, she ran at full speed, fell behind, and was teleported back at about 55 m, twice.

Found not working, and not fixed here:

- **Companion horse riding.** With Geralt mounted, the mod tries to put Triss on a horse and stops with `MCME horse: RiderData is unavailable for Triss` in the script log. The game has given her no rider data to pair with a horse. The same happened with a Triss removed and spawned again in that session, so it is not left over from an old save. Before these fixes the mod never got as far as trying. She follows on foot and teleports instead.

Not checked: boats, any companion other than Triss, and more than one companion at once.

## Watching the script log

This is how the faults were found. Start the game with `-debugscripts`:

```sh
steam -applaunch 292030 -debugscripts
```

The log is `Documents/The Witcher 3/scriptslog.txt` inside the game's Proton prefix (`steamapps/compatdata/292030/pfx/drive_c/users/steamuser/`). MCMR writes to the channel `ModSpawnCompanions` and the add-on to `MCM_AR`:

```sh
grep "ModSpawnCompanions\|MCM_AR" scriptslog.txt
```

A healthy load has `Started following and made code 1` (or higher) and, on an affected setup, the lines beginning `local patch:`. `made code -1` or `made code 0` with nothing after it is the second fault. No `Player obj FOUND` or `Player obj NOT FOUND` line at all is the first.

## Files

- `mcmr-local-fixes.patch`: the changes, as one unified diff against scripts with Unix line endings.
- `manifest.tsv`: for each script, the mod folder, the path under `content/scripts`, and the SHA-256 of the original and of the patched file.
- `apply.py`: applies, checks, or reverts.
