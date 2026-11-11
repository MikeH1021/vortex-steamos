extends SceneTree

## Drives the shell through its main paths without a controller and prints
## what happened. Any "SCRIPT ERROR" in the output is a failure.
##
##   eval "$(scripts/dev-sandbox.sh)"      # fresh pretend Steam and Nexus
##   cargo build -p vortex-cli
##   godot --headless --path ui --script ../scripts/ui-smoke.gd
##
## It expects the fresh sandbox: several games, no mods yet, and a canned
## Nexus that signs in as a Premium account.

func focus_key(shell) -> String:
	var owner = shell.get_viewport().gui_get_focus_owner()
	return "none" if owner == null else str(owner.get_meta("key", owner.get_class()))


func press(action: String) -> void:
	var event := InputEventAction.new()
	event.action = action
	event.pressed = true
	Input.parse_input_event(event)
	await process_frame
	await process_frame


func _initialize() -> void:
	var shell = load("res://scenes/main.tscn").instantiate()
	root.add_child(shell)
	await create_timer(3.0).timeout
	var ids: Array = shell._games.map(func(game): return game.id)
	print("games: ", ids)

	shell._show("games")
	await process_frame
	print("shelf focus: ", focus_key(shell))
	await press("ui_right")
	print("after right: ", focus_key(shell), " | ", shell._game_name.text)

	for index in ids.size():
		await shell._select_game(index)
		print(ids[index], ": view=", shell._view, " tabs=", shell._nav.map(func(tab): return tab.visible), " | ", shell._status_label.text)

	await shell._select_game(ids.find("skyrimse"))
	shell._step_section(1)
	shell._step_section(1)
	print("RB RB -> ", shell._view)

	shell._nexus_query = "campfire"
	await shell._search()
	print("search: hits=", shell._nexus_hits.size(), " focus=", focus_key(shell))
	await shell._open_files(shell._nexus_hits[0])
	print("files: ", shell._nexus_files.size(), " | ", shell._status_label.text)
	await shell._download(shell._nexus_files[0])
	print("download: view=", shell._view, " mods=", shell._mods.map(func(mod): return mod.id), " | ", shell._status_label.text)

	shell._show("plugins")
	var plugin = shell._plugins.filter(func(item): return not item.native)[0]
	await shell._toggle_plugin(plugin)
	print("plugin: ", shell._status_label.text)
	shell._show("mods")
	await shell._toggle_mod(shell._mods[0])
	print("toggle: ", shell._status_label.text, " focus=", focus_key(shell))

	shell.get_viewport().gui_release_focus()
	shell._jump(0)
	print("focus recovered: ", focus_key(shell))

	shell._show("mods")
	shell._open_mod(shell._mods[0])
	shell._ask_remove(shell._mods[0])
	await shell._confirmed()
	print("remove: mods=", shell._mods.size(), " | ", shell._status_label.text)

	shell._show("account")
	print("account rows: ", shell._rows.size(), " hints=", shell._hint_list())
	print("ok")
	quit()
