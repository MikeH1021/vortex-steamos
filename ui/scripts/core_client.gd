extends Node

## Runs vortex-cli and returns its JSON. The shell stays a controller UI.
## The core owns Steam, Proton, downloads, and plugin files.
##
## Every call runs on a worker thread. A multi-gigabyte download or a large
## deploy must never freeze the picture, least of all in Game Mode where a
## frozen app looks like a crash.

var _cli := ""
var _busy := false


func _ready() -> void:
	_cli = _find_cli()
	_leave_steam_runtime()


## Steam starts a non-Steam game with its overlay preloaded and its own
## library folder first in line. This window is already running, so that is
## fine for us. Programs started from here are another matter: the overlay
## prints a complaint into the core's output, and a browser handed Steam's
## old libraries can fail to start. Give children the system's own settings.
func _leave_steam_runtime() -> void:
	if OS.has_environment("LD_PRELOAD"):
		OS.unset_environment("LD_PRELOAD")
	if OS.has_environment("SYSTEM_LD_LIBRARY_PATH"):
		OS.set_environment("LD_LIBRARY_PATH", OS.get_environment("SYSTEM_LD_LIBRARY_PATH"))
	if OS.has_environment("SYSTEM_PATH"):
		OS.set_environment("PATH", OS.get_environment("SYSTEM_PATH"))


func available() -> bool:
	return _cli != ""


func busy() -> bool:
	return _busy


## Run the CLI and wait for it without blocking the frame loop.
## The result always has "ok". On failure "error" holds what the CLI printed.
func run(args: PackedStringArray) -> Dictionary:
	if _cli == "":
		return {"ok": false, "error": "missing_cli"}
	while _busy:
		await get_tree().process_frame
	_busy = true
	var thread := Thread.new()
	thread.start(_execute.bind(_cli, args))
	while thread.is_alive():
		await get_tree().process_frame
	var result: Dictionary = thread.wait_to_finish()
	_busy = false
	return result


static func _execute(cli: String, args: PackedStringArray) -> Dictionary:
	var output: Array = []
	var code := OS.execute(cli, args, output, true)
	var text := "\n".join(output).strip_edges()
	if code != 0:
		return {"ok": false, "error": text if text != "" else "vortex-cli exited %s" % code}
	var parsed = _parse(text)
	match typeof(parsed):
		TYPE_DICTIONARY:
			parsed["ok"] = true
			return parsed
		TYPE_ARRAY:
			return {"ok": true, "items": parsed}
	return {"ok": false, "error": text if text != "" else "vortex-cli printed nothing"}


static func _parse(text: String) -> Variant:
	var json := JSON.new()
	if json.parse(text) == OK:
		return json.data
	# Warnings on stderr can surround the document. Take the outermost object.
	var start := text.find("{")
	var ending := text.rfind("}")
	if start == -1 or ending < start:
		return null
	if json.parse(text.substr(start, ending - start + 1)) == OK:
		return json.data
	return null


func _find_cli() -> String:
	var override := OS.get_environment("VORTEX_CLI")
	if override != "" and FileAccess.file_exists(override):
		return override
	# An installed build keeps the core next to the shell.
	var beside := OS.get_executable_path().get_base_dir().path_join("vortex-cli")
	if FileAccess.file_exists(beside):
		return beside
	if not OS.has_feature("editor") and not OS.has_feature("debug"):
		return ""
	# A source checkout uses whatever cargo built last.
	var app_dir := ProjectSettings.globalize_path("res://").path_join("..").simplify_path()
	var newest := ""
	var newest_time := 0
	for build in ["debug", "release"]:
		var candidate := app_dir.path_join("target").path_join(build).path_join("vortex-cli")
		if FileAccess.file_exists(candidate):
			var modified := FileAccess.get_modified_time(candidate)
			if modified >= newest_time:
				newest = candidate
				newest_time = modified
	return newest
