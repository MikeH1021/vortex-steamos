#!/usr/bin/env node
// Read the simple game extensions out of an upstream Vortex checkout.
//
// Vortex game extensions are JavaScript programs. Many of them only state
// facts: the game's name, its Steam app id, its executable, and the folder
// mods go in. This script runs each extension against a pretend Vortex that
// records what the extension registers, and writes those facts to
// crates/vortex-core/games/generated.json, which the engine loads.
//
// An extension is skipped when it registers its own installers, mod types,
// load order, or merges, because that logic has to be ported by hand. It is
// also skipped when it is only a stub or has no Steam app id. Skipped games
// are listed with the reason.
//
// Extensions written in TypeScript are read when the esbuild package can be
// found (npm install esbuild, or point NODE_PATH at one). Without it they are
// skipped.
//
//   node scripts/import-vortex-games.cjs [path to Vortex checkout] [--facts out.json]
//
// --facts also writes what could be read from every extension, including the
// ones that need a hand port. It is the starting point for porting one.

const fs = require("fs");
const path = require("path");
const Module = require("module");

const here = __dirname;
const out = path.join(here, "..", "crates", "vortex-core", "games", "generated.json");
const core = path.join(here, "..", "crates", "vortex-core", "games", "core.json");
const args = process.argv.slice(2);
const factsAt = args.indexOf("--facts");
const factsPath = factsAt === -1 ? undefined : path.resolve(args[factsAt + 1]);
if (factsAt !== -1) args.splice(factsAt, 2);
const vortex = path.resolve(args[0] || path.join(here, "..", "..", "Vortex"));
let esbuild;
try {
  esbuild = require("esbuild");
} catch {
  esbuild = undefined;
}
const os = require("os");
const built = fs.mkdtempSync(path.join(os.tmpdir(), "vortex-import-"));
process.on("exit", () => fs.rmSync(built, { recursive: true, force: true }));

// The file to run for an extension. TypeScript is bundled to plain JavaScript first.
function entryOf(folder) {
  const plain = path.join(folder, "src", "index.js");
  if (fs.existsSync(plain)) return plain;
  const typed = ["index.ts", "index.tsx"].map((name) => path.join(folder, "src", name)).find(fs.existsSync);
  if (!typed || !esbuild) return undefined;
  const outfile = path.join(built, path.basename(folder) + ".cjs");
  esbuild.buildSync({
    entryPoints: [typed],
    outfile,
    bundle: true,
    platform: "node",
    format: "cjs",
    packages: "external",
    logLevel: "silent",
    loader: { ".scss": "empty", ".css": "empty", ".png": "empty", ".jpg": "empty", ".svg": "empty", ".json": "json" },
  });
  return outfile;
}
const gamesDir = path.join(vortex, "extensions", "games");

// Extensions reject promises nobody is listening to. That is not our problem.
process.on("unhandledRejection", () => {});

// Games that would import cleanly but need more than a mod folder.
const LEAVE_OUT = {
  nehrim: "an Oblivion total conversion with a plugin list, add it to core.json by hand",
};

// Vortex ids whose Nexus page has a different name.
const DOMAINS = { teso: "elderscrollsonline" };
// Registrations that mean the extension carries logic of its own.
const CUSTOM = ["registerInstaller", "registerModType", "registerLoadOrder", "registerLoadOrderPage", "registerMerge"];
// Windows folders an extension may build a path from. They map to the Proton prefix.
const BASES = { "<DOCUMENTS>": "documents", "<LOCALAPPDATA>": "appdata_local", "<APPDATA>": "appdata_roaming" };

// Something that can be called, read, or constructed and never complains.
function anything() {
  const target = function () {};
  return new Proxy(target, {
    get(_, key) {
      if (key === "then" || key === Symbol.toPrimitive || key === Symbol.iterator) return undefined;
      if (key === "toString" || key === "valueOf") return () => "";
      return anything();
    },
    apply: () => anything(),
    construct: () => anything(),
  });
}

function pretendVortex(seen) {
  const never = { then: () => never, catch: () => never, tap: () => never, finally: () => never };
  const helper = {
    findByAppId(ids) {
      // Without a store named, the list mixes Steam, GOG, and Epic ids.
      // Steam's are the short numbers.
      for (const id of [].concat(ids)) seen.steam.push(String(id));
      return never;
    },
    findByName: () => never,
  };
  const folders = { documents: "<DOCUMENTS>", localAppData: "<LOCALAPPDATA>", appData: "<APPDATA>" };
  const util = new Proxy({}, {
    get(_, key) {
      if (key === "GameStoreHelper" || key === "steam") return helper;
      if (key === "getVortexPath") return (name) => folders[name] || "<OTHER>";
      return anything();
    },
  });
  return new Proxy({}, { get: (_, key) => (key === "util" ? util : anything()) });
}

function load(entry, seen) {
  const api = pretendVortex(seen);
  const realLoad = Module._load;
  Module._load = function (request, parent, isMain) {
    if (request === "path") return path.posix;
    if (request.startsWith(".") || path.isAbsolute(request)) return realLoad.call(this, request, parent, isMain);
    if (request === "@nexusmods/vortex-api" || request === "vortex-api") return api;
    if (request === "bluebird") return Promise;
    return anything();
  };
  try {
    for (const key of Object.keys(require.cache)) {
      if (key.startsWith(gamesDir) || key.startsWith(built)) delete require.cache[key];
    }
    return require(entry);
  } finally {
    Module._load = realLoad;
  }
}

function attempt(run) {
  try {
    const value = run();
    return typeof value === "string" ? value : undefined;
  } catch {
    return undefined;
  }
}

// Run the extension as if on `platform` and return what it registered.
function register(entry, platform) {
  const seen = { steam: [], custom: new Set(), games: [], stub: false };
  const context = new Proxy({}, {
    get(_, key) {
      if (key === "registerGame") return (game) => seen.games.push(game);
      if (key === "registerGameStub") return () => { seen.stub = true; };
      if (CUSTOM.includes(key)) return () => seen.custom.add(key);
      if (key === "once" || key === "onceAsync") return () => {};
      return anything();
    },
  });
  process.env.LOCALAPPDATA = "<LOCALAPPDATA>";
  process.env.APPDATA = "<APPDATA>";
  const real = Object.getOwnPropertyDescriptor(process, "platform");
  Object.defineProperty(process, "platform", { value: platform, configurable: true });
  try {
    const loaded = load(entry, seen);
    (loaded.default || loaded)(context);
    seen.executables = seen.games.map((game) =>
      attempt(() => (typeof game.executable === "function" ? game.executable() : game.executable)));
  } finally {
    Object.defineProperty(process, "platform", real);
  }
  return seen;
}

function readExtension(folder) {
  let entry;
  try {
    entry = entryOf(folder);
  } catch (err) {
    return [{ reason: `could not be built (${String(err.message).split("\n")[0]})` }];
  }
  if (!entry) return [{ reason: "written in TypeScript, and esbuild is not installed" }];
  let seen;
  let native;
  try {
    seen = register(entry, "win32");
    // Many of these games have a Linux build with a differently named program.
    native = register(entry, "linux").executables;
  } catch (err) {
    return [{ reason: `could not be run (${String(err.message).split("\n")[0]})` }];
  }
  if (seen.stub) return [{ reason: "a stub that points at a community extension" }];
  if (seen.games.length === 0) return [{ reason: "registers no game" }];
  const custom = [...seen.custom].sort();
  return seen.games.map((game, index) => {
    const result = readGame(game, seen, seen.executables[index], native[index]);
    if (custom.length === 0) return result;
    // The facts are still worth having. The rules have to be ported.
    return { label: result.label, facts: result.game, custom, reason: `needs a hand port (${custom.join(", ")})` };
  });
}

function readGame(game, seen, executable, nativeExecutable) {
  const GAME = "<GAME>";
  let modPath = attempt(() => (typeof game.queryModPath === "function" ? game.queryModPath(GAME) : game.queryModPath));
  seen.steam.length = 0;
  try {
    if (typeof game.queryPath === "function") game.queryPath();
  } catch {
    // Only called so findByAppId sees the app id.
  }
  if (typeof game.id !== "string" || typeof game.name !== "string") return { reason: "id or name is computed" };
  const label = game.id;
  if (LEAVE_OUT[game.id]) return { label, reason: LEAVE_OUT[game.id] };
  if (!executable) return { label, reason: "executable is computed" };
  if (modPath === undefined) return { label, reason: "mod folder is computed" };

  const ids = [];
  const add = (value) => {
    const text = String(value == null ? "" : value);
    if (/^\d{1,8}$/.test(text) && !ids.includes(Number(text))) ids.push(Number(text));
  };
  add(game.details && game.details.steamAppId);
  add(game.environment && game.environment.SteamAPPId);
  for (const item of (game.queryArgs && game.queryArgs.steam) || []) add(item && item.id);
  seen.steam.forEach(add);
  if (ids.length === 0) return { label, reason: "no Steam app id" };

  modPath = modPath.replace(/\\/g, "/");
  let base = "game";
  if (modPath === GAME || modPath.startsWith(GAME + "/")) {
    modPath = modPath.slice(GAME.length + 1);
  } else {
    const marker = Object.keys(BASES).find((key) => modPath.startsWith(key));
    if (marker) {
      base = BASES[marker];
      modPath = modPath.slice(marker.length + 1);
    }
  }
  if (modPath.includes("<") || modPath.includes("..") || /^[A-Za-z]:/.test(modPath) || modPath.startsWith("/")) {
    return { label, reason: "mod folder is outside the game and the usual Windows folders" };
  }
  modPath = modPath.split("/").filter((part) => part && part !== ".").join("/");
  const merge = typeof game.mergeMods === "boolean" ? game.mergeMods : undefined;
  if (merge === undefined) return { label, reason: "mod folder naming is computed (mergeMods)" };

  const page = game.details && typeof game.details.nexusPageId === "string" ? game.details.nexusPageId : undefined;
  const entry = {
    id: game.id,
    name: game.name.replace(/\s+/g, " ").trim(),
    steam: ids,
    executable: executable.replace(/\\/g, "/"),
    mod_path: { base, path: modPath },
    nexus_domain: page || DOMAINS[game.id] || game.id,
    merge_mods: merge,
  };
  if (nativeExecutable && nativeExecutable !== executable) {
    entry.native_executable = nativeExecutable.replace(/\\/g, "/");
  }
  return { game: entry };
}

if (!fs.existsSync(gamesDir)) {
  console.error(`${gamesDir} is not there. Pass the path to a Vortex checkout.`);
  process.exit(1);
}
const byHand = new Set();
for (const file of [core, path.join(path.dirname(core), "ported.json")]) {
  if (fs.existsSync(file)) JSON.parse(fs.readFileSync(file, "utf8")).forEach((game) => byHand.add(game.id));
}
const games = [];
const skipped = [];
const facts = [];
for (const name of fs.readdirSync(gamesDir).sort()) {
  const folder = path.join(gamesDir, name);
  if (!name.startsWith("game-") || !fs.statSync(folder).isDirectory()) continue;
  for (const result of readExtension(folder)) {
    const label = result.label ? `${name} (${result.label})` : name;
    facts.push({ extension: name, game: result.game || result.facts, custom: result.custom || [], reason: result.reason });
    const known = result.game || result.facts;
    if (byHand.has(known ? known.id : result.label)) skipped.push([label, "defined by hand"]);
    else if (!result.game) skipped.push([label, result.reason]);
    else games.push(result.game);
  }
}
games.sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, JSON.stringify(games, null, 2) + "\n");
for (const game of games) {
  const where = game.mod_path.base === "game" ? game.mod_path.path || "the game folder" : `${game.mod_path.base}: ${game.mod_path.path}`;
  const linux = game.native_executable ? `, Linux build ${game.native_executable}` : "";
  console.log(`  + ${game.name}  (${game.id}, Steam ${game.steam.join("/")}, ${game.executable}${linux}, mods in ${where}${game.merge_mods ? "" : ", one folder per mod"})`);
}
console.log(`\n${games.length} games written to ${path.relative(path.join(here, ".."), out)}\n`);
for (const [name, reason] of skipped) console.log(`  - ${name}: ${reason}`);
if (factsPath) fs.writeFileSync(factsPath, JSON.stringify(facts, null, 2) + "\n");
