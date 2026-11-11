extends Control

## Controller-first shell. Everything that touches Steam, Proton, Nexus, or the
## game folder goes through vortex-cli (see core_client.gd).
##
## The layout follows Steam's Big Picture: tabs across the top that LB and RB
## step through, a shelf of game covers, lists that the stick walks one row at
## a time, and a footer that always says what each button does.
##
## A view is one screen. Four of them are tabs. The rest are reached from a
## tab and B returns to it.

const CoreClient := preload("res://scripts/core_client.gd")

const SECTIONS := ["games", "mods", "plugins", "downloads"]
const SECTION_NAMES := ["Games", "Mods", "Plugins", "Downloads"]
const PARENT := {
	"mod": "mods", "plugin": "plugins", "search": "downloads", "files": "search",
	"account": "downloads", "wizard": "downloads", "download": "downloads",
}
const DESKTOP_ID := "vortex-steamos.desktop"
const API_KEY_PAGE := "https://next.nexusmods.com/settings/api-keys"

const BG := Color("0e141b")
const BAR := Color("171d25")
const CARD := Color("1f2733")
const CARD_ON := Color("3d4450")
const TEXT := Color("ffffff")
const MUTED := Color("9099a3")
const ACCENT := Color("1a9fff")
const GOOD := Color("5ac85a")
const BAD := Color("ff6b5e")
const EDGE := 48


## A controller button drawn the way Steam draws them in its footer.
class Glyph extends Control:
	var text := "A"

	func _init(label: String) -> void:
		text = label
		custom_minimum_size = Vector2(46 if label.length() > 1 else 28, 28)
		mouse_filter = MOUSE_FILTER_IGNORE
		size_flags_vertical = SIZE_SHRINK_CENTER

	func _draw() -> void:
		var box := StyleBoxFlat.new()
		box.bg_color = Color.WHITE
		box.set_corner_radius_all(14 if text.length() == 1 else 8)
		box.draw(get_canvas_item(), Rect2(Vector2.ZERO, size))
		var font := get_theme_default_font()
		var height := 15
		var baseline := size.y / 2.0 + font.get_ascent(height) / 2.0 - 2.0
		draw_string(font, Vector2(0, baseline), text, HORIZONTAL_ALIGNMENT_CENTER, size.x, height, Color("0e141b"))


var _core: Node
var _games: Array = []
var _selected := -1
var _mods: Array = []
var _plugins: Array = []
var _pending: Array = []
var _failed: Array = []
var _plugin_warning := ""
var _incoming := ""
var _conflicts := 0
var _account: Dictionary = {}
var _account_known := false
var _link_handler := "unknown"

var _view := "games"
var _busy := false
var _focus_key := ""
var _target := {}
var _confirm := {}
var _rows: Array[Control] = []
var _progress_path := ""
var _progress_clock := 0.0
var _busy_text := ""

var _nexus_query := ""
var _nexus_hits: Array = []
var _nexus_mod := {}
var _nexus_files: Array = []
var _thumbs := {}
var _thumb_wanted := ""

var _wizard_path := ""
var _wizard_name := ""
var _wizard_plan: Dictionary = {}
var _wizard_step := 0
var _wizard_choices: Array = []

var _nav: Array[Button] = []
var _content: VBoxContainer
var _title: Label
var _status_label: Label
var _hints: HBoxContainer
var _game_label: Label
var _account_label: Label
var _hero: TextureRect
var _hero_cache := {}
var _game_detail: Label
var _game_name: Label
var _overlay: Control
var _overlay_label: Label
var _overlay_bar: ProgressBar
var _side_text: Label
var _side_image: TextureRect
var _http: HTTPRequest


func _ready() -> void:
	var links := _links_from_arguments()
	if not links.is_empty() and _another_instance_running():
		# The running window picks the link up from the queue.
		get_tree().quit()
		return
	_write_pid()
	_install_actions()
	_apply_window()
	_core = CoreClient.new()
	add_child(_core)
	_http = HTTPRequest.new()
	_http.timeout = 15.0
	_http.request_completed.connect(_on_thumbnail)
	add_child(_http)
	_build()
	var poll := Timer.new()
	poll.wait_time = 1.0
	poll.autostart = true
	poll.timeout.connect(_poll_links)
	add_child(poll)
	await _startup()
	await _apply_test_hooks()


func _notification(what: int) -> void:
	if what == NOTIFICATION_WM_CLOSE_REQUEST or what == NOTIFICATION_PREDELETE:
		var pid := _pid_path()
		if FileAccess.file_exists(pid):
			DirAccess.remove_absolute(pid)


func _process(delta: float) -> void:
	if not _busy or _progress_path == "":
		return
	_progress_clock += delta
	if _progress_clock < 0.2:
		return
	_progress_clock = 0.0
	if not FileAccess.file_exists(_progress_path):
		return
	var parsed = JSON.parse_string(FileAccess.get_file_as_string(_progress_path))
	if typeof(parsed) != TYPE_DICTIONARY:
		return
	var received := float(parsed.get("received", 0))
	var total := float(parsed.get("total", 0))
	if total > 0.0:
		_overlay_bar.visible = true
		_overlay_bar.value = 100.0 * received / total
		_overlay_label.text = "%s\n%s of %s" % [_busy_text, _bytes(received), _bytes(total)]
	elif received > 0.0:
		_overlay_label.text = "%s\n%s" % [_busy_text, _bytes(received)]


# --- startup -----------------------------------------------------------------

func _startup() -> void:
	if not _core.available():
		_show("games")
		_set_status("The core program vortex-cli is missing. Reinstall the app, or run cargo build -p vortex-cli in a source checkout.", true)
		return
	var found: Dictionary = await _call(["discover"], "Looking for your games…")
	_games = found.get("games", []) if found.get("ok", false) else []
	if not found.get("ok", false):
		_show("games")
		_set_status(str(found.get("error", "Could not read Steam libraries.")), true)
		return
	if _games.is_empty():
		_show("games")
		var roots: Array = found.get("roots", [])
		if roots.is_empty():
			_set_status("No Steam install found.")
		else:
			_set_status("Steam is here, and none of the supported games is installed.")
		return
	var last := str(_settings().get("last_game", ""))
	_selected = 0
	for index in _games.size():
		if str(_games[index].get("id", "")) == last:
			_selected = index
	_refresh_account()
	await _sync("mods" if last != "" else "games")
	var warnings: Array = found.get("warnings", [])
	if not warnings.is_empty() and last == "":
		_set_status(str(warnings[0]))


func _refresh_account() -> void:
	var result: Dictionary = await _core.run(PackedStringArray(["nexus", "status"]))
	if result.get("ok", false):
		_account = result
		_account_known = true
		if _view == "downloads" and not _busy:
			_paint()


func _apply_test_hooks() -> void:
	# Used to capture screenshots: VORTEX_VIEW=search:campfire VORTEX_SHOT=out.png
	var wanted := OS.get_environment("VORTEX_VIEW")
	if wanted != "":
		var parts := wanted.split(":", true, 1)
		var argument := parts[1] if parts.size() > 1 else ""
		match parts[0]:
			"search":
				_nexus_query = argument
				_show("search")
				if argument != "":
					await _search()
			"files":
				await _open_files({"mod_id": int(argument), "name": "Mod"})
			"mod":
				if not _mods.is_empty():
					_open_mod(_mods[0])
			"plugin":
				for plugin in _plugins:
					if not plugin.get("native", false):
						_open_plugin(plugin)
						break
			"wizard":
				if not _pending.is_empty():
					await _open_wizard(_pending[0])
			"busy":
				_begin_busy("Downloading Campfire…")
				_overlay_bar.visible = true
				_overlay_bar.value = 62.0
				_overlay_label.text = "Downloading Campfire…\n148 MB of 239 MB"
			_:
				_show(parts[0])
	var shot := OS.get_environment("VORTEX_SHOT")
	if shot != "":
		var delay := OS.get_environment("VORTEX_SHOT_DELAY")
		await get_tree().create_timer(float(delay) if delay != "" else 0.6).timeout
		get_viewport().get_texture().get_image().save_png(shot)
		get_tree().quit()


# --- window and input ----------------------------------------------------------

func _game_mode() -> bool:
	# Game Mode on a Deck or Bazzite, or Big Picture on a desktop.
	return OS.get_environment("SteamDeck") == "1" or OS.get_environment("GAMESCOPE_WAYLAND_DISPLAY") != "" or OS.get_environment("SteamTenfoot") == "1"


func _apply_window() -> void:
	if _game_mode():
		DisplayServer.window_set_mode(DisplayServer.WINDOW_MODE_FULLSCREEN)


func _install_actions() -> void:
	_bind("section_prev", KEY_Q, JOY_BUTTON_LEFT_SHOULDER)
	_bind("section_next", KEY_E, JOY_BUTTON_RIGHT_SHOULDER)
	_bind("toggle", KEY_X, JOY_BUTTON_X)
	_bind("details", KEY_Y, JOY_BUTTON_Y)
	# The triggers jump through long lists.
	for page in [["page_up", KEY_PAGEUP, JOY_AXIS_TRIGGER_LEFT], ["page_down", KEY_PAGEDOWN, JOY_AXIS_TRIGGER_RIGHT]]:
		InputMap.add_action(page[0], 0.5)
		var key := InputEventKey.new()
		key.keycode = page[1]
		InputMap.action_add_event(page[0], key)
		var trigger := InputEventJoypadMotion.new()
		trigger.axis = page[2]
		trigger.axis_value = 1.0
		InputMap.action_add_event(page[0], trigger)


func _bind(action: StringName, key: Key, button: JoyButton) -> void:
	if not InputMap.has_action(action):
		InputMap.add_action(action, 0.5)
	var key_event := InputEventKey.new()
	key_event.keycode = key
	InputMap.action_add_event(action, key_event)
	var joy_event := InputEventJoypadButton.new()
	joy_event.button_index = button
	InputMap.action_add_event(action, joy_event)


func _input(event: InputEvent) -> void:
	# Nothing may be pressed while the core is working on the game folder.
	if _busy and (event is InputEventKey or event is InputEventJoypadButton or event is InputEventJoypadMotion or event is InputEventMouseButton):
		get_viewport().set_input_as_handled()


func _unhandled_input(event: InputEvent) -> void:
	if _busy:
		return
	var handled := true
	if event.is_action_pressed("ui_cancel"):
		_back()
	elif event.is_action_pressed("section_prev"):
		if _view == "wizard":
			_wizard_back()
		else:
			_step_section(-1)
	elif event.is_action_pressed("section_next"):
		if _view == "wizard":
			_wizard_advance()
		else:
			_step_section(1)
	elif event.is_action_pressed("toggle"):
		_quick_toggle()
	elif event.is_action_pressed("details"):
		_show_details()
	elif event.is_action_pressed("page_up"):
		_jump(-5)
	elif event.is_action_pressed("page_down"):
		_jump(5)
	elif get_viewport().gui_get_focus_owner() == null and _is_move(event):
		# A mouse click or a closed keyboard can leave nothing highlighted.
		# The next push of the stick picks the list back up.
		_jump(0)
	else:
		handled = false
	if handled:
		get_viewport().set_input_as_handled()


func _is_move(event: InputEvent) -> bool:
	for action in ["ui_up", "ui_down", "ui_left", "ui_right", "ui_accept"]:
		if event.is_action_pressed(action):
			return true
	return false


## Move the highlight `by` rows, stopping at either end.
func _jump(by: int) -> void:
	var rows := _focusable()
	if rows.is_empty():
		return
	var at := rows.find(get_viewport().gui_get_focus_owner())
	rows[clampi(maxi(at, 0) + by, 0, rows.size() - 1)].grab_focus()


func _focusable() -> Array[Control]:
	var rows: Array[Control] = []
	for row in _rows:
		if is_instance_valid(row) and not (row is Button and row.disabled):
			rows.append(row)
	return rows


func _step_section(by: int) -> void:
	var index := SECTIONS.find(_section_of(_view))
	for step in SECTIONS.size():
		index = posmod(index + by, SECTIONS.size())
		if _tab_shown(index):
			break
	_show(SECTIONS[index])


## Plugins only exists for games that have a plugin list.
func _tab_shown(index: int) -> bool:
	return SECTIONS[index] != "plugins" or _game().get("plugin_format") != null


func _section_of(view: String) -> String:
	var current := view
	if current == "confirm":
		current = str(_confirm.get("back", "games"))
	while PARENT.has(current):
		current = PARENT[current]
	return current


func _back() -> void:
	match _view:
		"wizard":
			_wizard_back()
		"confirm":
			_show(str(_confirm.get("back", "games")))
		"games":
			pass
		_:
			_show(PARENT.get(_view, "games"))


func _quick_toggle() -> void:
	var row := get_viewport().gui_get_focus_owner()
	if row == null or not row.has_meta("item"):
		return
	var item: Dictionary = row.get_meta("item")
	match _view:
		"mods":
			_toggle_mod(item)
		"plugins":
			_toggle_plugin(item)
		"downloads":
			_ask_discard(item)
		"wizard":
			if row is Button and not row.disabled:
				row.pressed.emit()


func _show_details() -> void:
	var row := get_viewport().gui_get_focus_owner()
	if row != null and row.has_meta("details"):
		_set_status(str(row.get_meta("details")))


# --- core calls ----------------------------------------------------------------

func _call(args: Array, label: String, progress := "") -> Dictionary:
	_begin_busy(label, progress)
	var result: Dictionary = await _core.run(PackedStringArray(args))
	_end_busy()
	return result


func _begin_busy(label: String, progress := "") -> void:
	_busy = true
	_busy_text = label
	_progress_path = progress
	_progress_clock = 0.0
	if progress != "" and FileAccess.file_exists(progress):
		DirAccess.remove_absolute(progress)
	_overlay_label.text = label
	_overlay_bar.visible = false
	_overlay_bar.value = 0.0
	_overlay.visible = true
	get_viewport().gui_release_focus()


func _end_busy() -> void:
	_busy = false
	_overlay.visible = false
	if _progress_path != "" and FileAccess.file_exists(_progress_path):
		DirAccess.remove_absolute(_progress_path)
	_progress_path = ""


func _game() -> Dictionary:
	if _selected < 0 or _selected >= _games.size():
		return {}
	return _games[_selected]


func _game_id() -> String:
	return str(_game().get("id", ""))


## Install what is waiting, deploy, and reload everything for the current game.
func _sync(then_view := "", note := "") -> Dictionary:
	if _game().is_empty():
		_show("games")
		return {}
	var result: Dictionary = await _call(["sync", "--game", _game_id()], "Deploying mods…")
	if not _take_state(result):
		_show(then_view if then_view != "" else _view)
		_set_status(str(result.get("error", "Could not read this game.")), true)
		return result
	_show(then_view if then_view != "" else _view)
	var installed: Array = result.get("installed", [])
	if note != "":
		_set_status(note)
	elif not _failed.is_empty():
		_set_status("%s could not be installed. It is waiting in Downloads." % _failed[0].get("name", "An archive"), true)
	elif not installed.is_empty():
		_set_status("Installed %s." % _name_list(installed))
	return result


func _take_state(result: Dictionary) -> bool:
	if not result.get("ok", false):
		return false
	_mods = result.get("mods", _mods)
	_plugins = result.get("plugins", _plugins)
	if result.has("deploy"):
		_conflicts = int(result["deploy"].get("conflicts", 0))
	if result.has("pending"):
		_pending = result["pending"]
		_failed = result.get("failed", [])
		_incoming = str(result.get("incoming", _incoming))
	if result.has("plugin_warning"):
		var warning = result["plugin_warning"]
		_plugin_warning = "" if warning == null else str(warning)
	return true


func _name_list(entries: Array) -> String:
	if entries.size() == 1:
		return str(entries[0].get("name", "1 mod"))
	return "%d mods" % entries.size()


# --- layout ----------------------------------------------------------------------

func _build() -> void:
	var background := ColorRect.new()
	background.color = BG
	background.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	background.mouse_filter = MOUSE_FILTER_IGNORE
	add_child(background)

	# The game's backdrop from Steam's library, faded into the background.
	_hero = TextureRect.new()
	_hero.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	_hero.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	_hero.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_COVERED
	_hero.modulate = Color(1, 1, 1, 0.0)
	_hero.mouse_filter = MOUSE_FILTER_IGNORE
	add_child(_hero)
	var fade := Gradient.new()
	fade.set_color(0, Color(BG, 0.55))
	fade.set_color(1, BG)
	fade.set_offset(1, 0.72)
	var fade_texture := GradientTexture2D.new()
	fade_texture.gradient = fade
	fade_texture.fill_from = Vector2(0, 0)
	fade_texture.fill_to = Vector2(0, 1)
	var shade := TextureRect.new()
	shade.texture = fade_texture
	shade.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	shade.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	shade.stretch_mode = TextureRect.STRETCH_SCALE
	shade.mouse_filter = MOUSE_FILTER_IGNORE
	add_child(shade)

	var root := VBoxContainer.new()
	root.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	root.add_theme_constant_override("separation", 0)
	add_child(root)

	# Header: tabs in the middle, the shoulder buttons that step through them
	# on either side, and who and what is active on the right.
	var header := PanelContainer.new()
	header.custom_minimum_size = Vector2(0, 68)
	var header_style := _panel(Color(BAR, 0.92), 0, 0, 0)
	header_style.content_margin_left = EDGE
	header_style.content_margin_right = EDGE
	header.add_theme_stylebox_override("panel", header_style)
	root.add_child(header)
	var bar := HBoxContainer.new()
	bar.add_theme_constant_override("separation", 14)
	header.add_child(bar)
	var word := _label("VORTEX", 20, TEXT)
	word.size_flags_horizontal = SIZE_SHRINK_BEGIN
	word.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	word.size_flags_vertical = SIZE_EXPAND_FILL
	bar.add_child(word)
	bar.add_child(_gap_wide(26))
	bar.add_child(Glyph.new("LB"))
	for index in SECTIONS.size():
		var tab := Button.new()
		tab.text = SECTION_NAMES[index].to_upper()
		tab.focus_mode = FOCUS_NONE
		tab.custom_minimum_size = Vector2(0, 68)
		tab.add_theme_font_size_override("font_size", 18)
		tab.pressed.connect(_show.bind(SECTIONS[index]))
		bar.add_child(tab)
		_nav.append(tab)
	bar.add_child(Glyph.new("RB"))
	var spacer := Control.new()
	spacer.size_flags_horizontal = SIZE_EXPAND_FILL
	bar.add_child(spacer)
	_game_label = _label("", 16, TEXT)
	_game_label.size_flags_horizontal = SIZE_SHRINK_END
	_game_label.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	_game_label.size_flags_vertical = SIZE_EXPAND_FILL
	bar.add_child(_game_label)
	_account_label = _label("", 16, MUTED)
	_account_label.size_flags_horizontal = SIZE_SHRINK_END
	_account_label.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	_account_label.size_flags_vertical = SIZE_EXPAND_FILL
	bar.add_child(_account_label)

	var heading := _margin(EDGE, 26, EDGE, 6)
	root.add_child(heading)
	var heading_box := VBoxContainer.new()
	heading_box.add_theme_constant_override("separation", 6)
	heading.add_child(heading_box)
	_title = _label("", 30, TEXT)
	_title.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	heading_box.add_child(_title)
	_status_label = _label("", 18, MUTED)
	_status_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_status_label.custom_minimum_size = Vector2(0, 26)
	heading_box.add_child(_status_label)

	var scroll := ScrollContainer.new()
	scroll.size_flags_vertical = SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	scroll.follow_focus = true
	root.add_child(scroll)
	var margin := _margin(EDGE, 10, EDGE, 16)
	margin.size_flags_horizontal = SIZE_EXPAND_FILL
	scroll.add_child(margin)
	_content = VBoxContainer.new()
	_content.size_flags_horizontal = SIZE_EXPAND_FILL
	_content.add_theme_constant_override("separation", 8)
	margin.add_child(_content)

	# Footer: what each button does on this screen, as Steam shows it.
	var footer := PanelContainer.new()
	footer.custom_minimum_size = Vector2(0, 56)
	var footer_style := _panel(BAR, 0, 0, 0)
	footer_style.content_margin_left = EDGE
	footer_style.content_margin_right = EDGE
	footer.add_theme_stylebox_override("panel", footer_style)
	root.add_child(footer)
	_hints = HBoxContainer.new()
	_hints.alignment = BoxContainer.ALIGNMENT_END
	_hints.add_theme_constant_override("separation", 10)
	footer.add_child(_hints)

	_overlay = ColorRect.new()
	_overlay.color = Color(0.03, 0.05, 0.07, 0.8)
	_overlay.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	_overlay.mouse_filter = MOUSE_FILTER_STOP
	_overlay.visible = false
	add_child(_overlay)
	var center := CenterContainer.new()
	center.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	_overlay.add_child(center)
	var card := PanelContainer.new()
	card.custom_minimum_size = Vector2(560, 0)
	card.add_theme_stylebox_override("panel", _panel(CARD, 0, 6, 32))
	center.add_child(card)
	var card_box := VBoxContainer.new()
	card_box.add_theme_constant_override("separation", 18)
	card.add_child(card_box)
	_overlay_label = _label("", 22, TEXT)
	_overlay_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_overlay_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	card_box.add_child(_overlay_label)
	_overlay_bar = ProgressBar.new()
	_overlay_bar.custom_minimum_size = Vector2(0, 10)
	_overlay_bar.show_percentage = false
	_overlay_bar.add_theme_stylebox_override("background", _panel(BAR, 0, 5, 0))
	_overlay_bar.add_theme_stylebox_override("fill", _panel(ACCENT, 0, 5, 0))
	card_box.add_child(_overlay_bar)


func _show(view: String) -> void:
	if view != _view:
		_focus_key = ""
	_view = view
	_paint()


func _paint() -> void:
	for child in _content.get_children():
		_content.remove_child(child)
		child.queue_free()
	_rows.clear()
	_side_text = null
	_side_image = null
	_game_detail = null
	_game_name = null
	_thumb_wanted = ""
	var game := _game()
	_game_label.text = str(game.get("name", ""))
	var who := str(_account.get("name", "")).strip_edges()
	if _account.get("signed_in", false):
		_account_label.text = "·   Nexus: %s" % (who if who != "" else "signed in")
	else:
		_account_label.text = "·   Nexus: not signed in" if _account_known else ""
	_set_status("")
	_set_hero(game)
	match _view:
		"games": _paint_games()
		"mods": _paint_mods()
		"mod": _paint_mod()
		"plugins": _paint_plugins()
		"plugin": _paint_plugin()
		"downloads": _paint_downloads()
		"download": _paint_download()
		"search": _paint_search()
		"files": _paint_files()
		"account": _paint_account()
		"wizard": _paint_wizard()
		"confirm": _paint_confirm()
	_paint_nav()
	_paint_hints()
	_wire_focus()


func _set_hero(game: Dictionary) -> void:
	var art = game.get("art")
	var path := "" if art == null or art.get("hero") == null else str(art["hero"])
	if path != "" and not _hero_cache.has(path):
		var image := Image.load_from_file(path) if FileAccess.file_exists(path) else null
		_hero_cache[path] = ImageTexture.create_from_image(image) if image != null and not image.is_empty() else null
	var texture: Texture2D = _hero_cache.get(path)
	_hero.texture = texture
	_hero.modulate = Color(1, 1, 1, 0.42 if texture != null else 0.0)


func _paint_nav() -> void:
	var section := SECTIONS.find(_section_of(_view))
	for index in _nav.size():
		var tab := _nav[index]
		var selected := index == section
		tab.visible = _tab_shown(index)
		var box := StyleBoxFlat.new()
		box.bg_color = Color(0, 0, 0, 0)
		box.content_margin_left = 16
		box.content_margin_right = 16
		box.border_width_bottom = 3
		box.border_color = ACCENT if selected else Color(0, 0, 0, 0)
		for state in ["normal", "hover", "pressed", "focus"]:
			tab.add_theme_stylebox_override(state, box)
		for color_name in ["font_color", "font_pressed_color", "font_focus_color"]:
			tab.add_theme_color_override(color_name, TEXT if selected else MUTED)
		tab.add_theme_color_override("font_hover_color", TEXT)


## Every row names its neighbours, so the stick always goes where it looks
## like it should and never wanders off the list. A shelf runs left and right.
## Everything else runs up and down.
func _wire_focus() -> void:
	var rows := _focusable()
	var wanted: Control = null
	for index in rows.size():
		var row := rows[index]
		var own := row.get_path()
		var before := rows[index - 1].get_path() if index > 0 else own
		var after := rows[index + 1].get_path() if index < rows.size() - 1 else own
		var sideways: bool = row.get_meta("tile", false)
		row.focus_neighbor_left = before if sideways else own
		row.focus_neighbor_right = after if sideways else own
		row.focus_neighbor_top = own if sideways else before
		row.focus_neighbor_bottom = own if sideways else after
		row.focus_previous = before
		row.focus_next = after
		if _focus_key != "" and str(row.get_meta("key", "")) == _focus_key:
			wanted = row
	if _busy or rows.is_empty():
		return
	(wanted if wanted != null else rows[0]).grab_focus()


## What each button does on this screen, most important last so it sits at the
## right edge where Steam puts "Select".
func _hint_list() -> Array:
	var hints: Array = []
	match _view:
		"games":
			hints = [["Y", "Details"], ["A", "Open"]]
		"mods", "plugins":
			hints = [["LT", "RT", "Page"], ["X", "On / Off"], ["B", "Back"], ["A", "Options"]]
		"downloads":
			hints = [["X", "Discard"], ["B", "Back"], ["A", "Select"]]
		"wizard":
			hints = [["LB", "RB", "Step"], ["Y", "Details"], ["B", "Back"], ["A", "Choose"]]
		"search", "files":
			hints = [["Y", "Details"], ["B", "Back"], ["A", "Select"]]
		_:
			hints = [["B", "Back"], ["A", "Select"]]
	return hints


func _paint_hints() -> void:
	for child in _hints.get_children():
		_hints.remove_child(child)
		child.queue_free()
	for hint in _hint_list():
		for index in hint.size() - 1:
			_hints.add_child(Glyph.new(hint[index]))
		var label := _label(str(hint[-1]), 17, TEXT)
		label.size_flags_horizontal = SIZE_SHRINK_BEGIN
		label.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
		_hints.add_child(label)
		_hints.add_child(_gap_wide(18))


func _set_title(text: String) -> void:
	_title.text = text


func _set_status(text: String, error := false) -> void:
	_status_label.text = text
	_status_label.add_theme_color_override("font_color", BAD if error else MUTED)


# --- games -------------------------------------------------------------------------

func _paint_games() -> void:
	_set_title("Your games")
	if not _core.available():
		return
	if _games.is_empty():
		_note("Install a supported game in Steam, start it once so it sets itself up, then come back.\n\nSupported games include Skyrim, Fallout, Starfield, Baldur's Gate 3, The Witcher 3, Cyberpunk 2077, Stardew Valley, and many more.")
		var again := _row("Look again", "Search the Steam libraries for games")
		again.pressed.connect(_rescan)
		return
	_set_status("Pick the game you want to mod.")
	# A shelf of covers, like the one Steam opens on.
	var strip := ScrollContainer.new()
	strip.vertical_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	strip.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_SHOW_NEVER
	strip.follow_focus = true
	strip.custom_minimum_size = Vector2(0, 384)
	_content.add_child(strip)
	var shelf := HBoxContainer.new()
	shelf.add_theme_constant_override("separation", 18)
	strip.add_child(_pad(shelf, 14))
	for index in _games.size():
		shelf.add_child(_tile(_games[index], index))
	_game_name = _label("", 26, TEXT)
	_content.add_child(_game_name)
	_game_detail = _label("", 18, MUTED)
	_game_detail.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_content.add_child(_game_detail)
	if _focus_key == "" and _selected >= 0:
		_focus_key = str(_game_id())


## One game on the shelf: its cover from Steam, or its name when Steam has
## not downloaded a cover.
func _tile(game: Dictionary, index: int) -> Button:
	var tile := Button.new()
	tile.focus_mode = FOCUS_ALL
	tile.custom_minimum_size = Vector2(232, 348)
	tile.clip_contents = true
	var plain := _panel(CARD, 0, 6, 0)
	var ring := _panel(CARD_ON, 0, 6, 0)
	ring.set_border_width_all(4)
	ring.border_color = TEXT
	tile.add_theme_stylebox_override("normal", plain)
	tile.add_theme_stylebox_override("hover", plain)
	for state in ["pressed", "focus"]:
		tile.add_theme_stylebox_override(state, ring)
	var art = game.get("art")
	var cover := "" if art == null or art.get("capsule") == null else str(art["capsule"])
	var image := Image.load_from_file(cover) if cover != "" and FileAccess.file_exists(cover) else null
	if image != null and not image.is_empty():
		var picture := TextureRect.new()
		picture.texture = ImageTexture.create_from_image(image)
		picture.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
		picture.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_COVERED
		picture.set_anchors_and_offsets_preset(PRESET_FULL_RECT, PRESET_MODE_MINSIZE, 6)
		picture.mouse_filter = MOUSE_FILTER_IGNORE
		tile.add_child(picture)
	else:
		var name := _label(str(game.get("name", "Game")), 24, TEXT)
		name.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
		name.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
		name.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
		name.set_anchors_and_offsets_preset(PRESET_FULL_RECT, PRESET_MODE_MINSIZE, 18)
		tile.add_child(name)
	if index == _selected:
		var flag := _label("MODDING NOW", 13, TEXT)
		flag.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
		flag.add_theme_stylebox_override("normal", _panel(ACCENT, 0, 3, 5))
		flag.set_anchors_and_offsets_preset(PRESET_BOTTOM_WIDE, PRESET_MODE_MINSIZE, 12)
		tile.add_child(flag)
	tile.set_meta("tile", true)
	tile.set_meta("key", str(game.get("id", "")))
	var notes := str(game.get("notes", ""))
	tile.set_meta("details", notes if notes != "" else "Mods go in %s" % game.get("data_path", ""))
	tile.focus_entered.connect(_describe_game.bind(game))
	# The highlighted cover lifts a little, as it does on Steam's shelf.
	tile.pivot_offset = tile.custom_minimum_size / 2.0
	tile.focus_entered.connect(_lift.bind(tile, 1.05))
	tile.focus_exited.connect(_lift.bind(tile, 1.0))
	tile.pressed.connect(_select_game.bind(index))
	_rows.append(tile)
	return tile


func _lift(tile: Control, to: float) -> void:
	if not is_instance_valid(tile) or not tile.is_inside_tree():
		return
	tile.z_index = 1 if to > 1.0 else 0
	create_tween().tween_property(tile, "scale", Vector2(to, to), 0.12).set_ease(Tween.EASE_OUT)


func _describe_game(game: Dictionary) -> void:
	if _game_name == null:
		return
	var proton: Dictionary = game.get("proton", {})
	var extender: Dictionary = game.get("script_extender") if game.get("script_extender") != null else {}
	var facts: Array[String] = []
	# Only games with folders inside a Proton prefix have to run once first.
	var waits: bool = game.get("needs_first_launch", false) or (game.get("plugin_format") != null and not proton.get("prefix_exists", false))
	facts.append("Start it once from Steam first" if waits else "Ready to mod")
	if not extender.is_empty():
		facts.append("%s %s" % [extender.get("name", "Mod loader"), "installed" if extender.get("installed", false) else "not installed"])
	if game.get("native", false):
		facts.append("Linux build")
	facts.append(_short(str(game.get("library_path", ""))))
	_game_name.text = str(game.get("name", "Game"))
	_game_detail.text = "   ·   ".join(facts)
	_set_hero(game)


func _rescan() -> void:
	if _busy:
		return
	await _startup()


func _select_game(index: int) -> void:
	if _busy:
		return
	_selected = index
	_mods = []
	_plugins = []
	_pending = []
	_failed = []
	_save_setting("last_game", _game_id())
	await _sync("mods")


# --- mods ----------------------------------------------------------------------------

func _paint_mods() -> void:
	_set_title("Mods")
	if _game().is_empty():
		_note("Choose a game first.")
		return
	var notes := str(_game().get("notes", ""))
	if _mods.is_empty():
		_set_status("No mods yet.")
		_note("Mods you install show up here, and are linked into the game when they are on.")
		var find := _row("Find mods", "Search Nexus Mods or open the website")
		find.pressed.connect(_show.bind("downloads"))
		if notes != "":
			_heading("Good to know")
			_note(notes)
		return
	var enabled := 0
	for mod in _mods:
		if mod.get("enabled", false):
			enabled += 1
	var status := "%d of %d on." % [enabled, _mods.size()]
	if _conflicts > 0:
		status += "  %s from more than one mod. The mod lower in this list wins." % ("1 file comes" if _conflicts == 1 else "%d files come" % _conflicts)
	if not _pending.is_empty():
		status += "  %d download%s waiting in Downloads." % [_pending.size(), "" if _pending.size() == 1 else "s"]
	_set_status(status)
	for mod in _mods:
		var on: bool = mod.get("enabled", false)
		var row := _row(str(mod.get("name", mod.get("id", "Mod"))), _mod_facts(mod), "ON" if on else "OFF", on)
		row.set_meta("key", str(mod.get("id", "")))
		row.set_meta("item", mod)
		row.pressed.connect(_open_mod.bind(mod))
	if notes != "":
		_heading("Good to know")
		_note(notes)


func _mod_facts(mod: Dictionary) -> String:
	var facts: Array[String] = []
	var file := str(mod.get("file", "")).strip_edges()
	if file != "" and file.to_lower() != str(mod.get("name", "")).to_lower():
		facts.append(file)
	var version := str(mod.get("version", ""))
	if version != "":
		facts.append("v" + version.trim_prefix("v"))
	var files := int(mod.get("files", 0))
	if files > 0:
		facts.append("%d file%s" % [files, "" if files == 1 else "s"])
	if mod.get("nexus") != null:
		facts.append("Nexus Mods")
	return "  ·  ".join(facts)


func _open_mod(mod: Dictionary) -> void:
	_target = mod
	_show("mod")


func _paint_mod() -> void:
	var mod := _current_mod()
	if mod.is_empty():
		_show("mods")
		return
	var on: bool = mod.get("enabled", false)
	_set_title(str(mod.get("name", "Mod")))
	_set_status("%s  ·  %s" % ["On" if on else "Off", _mod_facts(mod)] if _mod_facts(mod) != "" else ("On" if on else "Off"))
	var toggle := _row("Turn off" if on else "Turn on", "Its files leave the game folder and stay installed" if on else "Link its files into the game folder")
	toggle.set_meta("key", "toggle")
	toggle.pressed.connect(_toggle_mod.bind(mod))
	var up := _row("Move up", "Mods lower in the list win when two change the same file")
	up.set_meta("key", "up")
	up.pressed.connect(_move_mod.bind(mod, -1))
	var down := _row("Move down", "Lets this mod win over the ones above it")
	down.set_meta("key", "down")
	down.pressed.connect(_move_mod.bind(mod, 1))
	if mod.get("nexus") != null:
		var page := _row("Open on Nexus Mods", "Description, updates, and requirements")
		page.set_meta("key", "page")
		page.pressed.connect(_open_mod_page.bind(mod["nexus"], false))
	var remove := _row("Remove", "Delete this mod and its downloaded archive")
	remove.set_meta("key", "remove")
	remove.pressed.connect(_ask_remove.bind(mod))


func _current_mod() -> Dictionary:
	for mod in _mods:
		if str(mod.get("id", "")) == str(_target.get("id", "")):
			return mod
	return {}


func _toggle_mod(mod: Dictionary) -> void:
	if _busy:
		return
	_remember_focus()
	var turning_on: bool = not mod.get("enabled", false)
	var result: Dictionary = await _call(["toggle", "--game", _game_id(), "--mod", str(mod.get("id", ""))], "Turning %s %s…" % [mod.get("name", "mod"), "on" if turning_on else "off"])
	_after_change(result, "%s is %s." % [mod.get("name", "Mod"), "on" if turning_on else "off"])


func _move_mod(mod: Dictionary, by: int) -> void:
	if _busy:
		return
	_remember_focus()
	var result: Dictionary = await _call(["move", "--game", _game_id(), "--mod", str(mod.get("id", "")), "--by", str(by)], "Reordering…")
	var place := 0
	for index in _mods.size():
		if result.get("ok", false) and str(result["mods"][index].get("id", "")) == str(mod.get("id", "")):
			place = index + 1
	_after_change(result, "%s is now number %d of %d." % [mod.get("name", "Mod"), place, _mods.size()])


func _ask_remove(mod: Dictionary) -> void:
	_confirm = {
		"back": "mod",
		"title": "Remove %s?" % mod.get("name", "this mod"),
		"body": "Its files are taken out of the game and deleted, along with the downloaded archive. Saves that depend on it may stop loading.",
		"yes": "Remove",
		"action": _remove_mod.bind(mod),
	}
	_show("confirm")


func _remove_mod(mod: Dictionary) -> void:
	var result: Dictionary = await _call(["remove", "--game", _game_id(), "--mod", str(mod.get("id", ""))], "Removing %s…" % mod.get("name", "mod"))
	_view = "mods"
	_focus_key = ""
	_after_change(result, "Removed %s." % mod.get("name", "the mod"))


func _after_change(result: Dictionary, note: String) -> void:
	if _take_state(result):
		_paint()
		_set_status(note)
	else:
		_paint()
		_set_status(str(result.get("error", "That did not work.")), true)


func _remember_focus() -> void:
	var row := get_viewport().gui_get_focus_owner()
	if row != null and row.has_meta("key"):
		_focus_key = str(row.get_meta("key"))


# --- plugins -------------------------------------------------------------------------

func _paint_plugins() -> void:
	_set_title("Plugins")
	if _game().is_empty():
		_note("Choose a game first.")
		return
	if _game().get("plugin_format") == null:
		_note("%s has no plugin list. Its mods load on their own once they are on, so there is nothing to order here." % _game().get("name", "This game"))
		return
	if _plugin_warning != "":
		_set_status(_plugin_warning)
		_note("The plugin list lives inside the game's Proton prefix, which Steam creates the first time the game runs.")
		return
	if _plugins.is_empty():
		_note("No plugins yet. Mods that add content bring .esp, .esm, or .esl files, and they are listed here in the order the game loads them.")
		return
	_set_status("The game loads these from top to bottom. A plugin lower down overrides the ones above it.")
	for index in _plugins.size():
		var plugin: Dictionary = _plugins[index]
		var native: bool = plugin.get("native", false)
		var on: bool = plugin.get("enabled", false)
		var row := _row(str(plugin.get("name", "")), "Part of the game" if native else "", "GAME" if native else ("ON" if on else "OFF"), on and not native)
		row.set_meta("key", str(plugin.get("name", "")))
		if native:
			row.pressed.connect(_set_status.bind("%s is part of the game. It stays on and loads first." % plugin.get("name", "")))
		else:
			row.set_meta("item", plugin)
			row.pressed.connect(_open_plugin.bind(plugin))


func _open_plugin(plugin: Dictionary) -> void:
	_target = plugin
	_show("plugin")


func _paint_plugin() -> void:
	var name := str(_target.get("name", ""))
	var plugin := {}
	var place := 0
	for index in _plugins.size():
		if str(_plugins[index].get("name", "")) == name:
			plugin = _plugins[index]
			place = index + 1
	if plugin.is_empty():
		_show("plugins")
		return
	var on: bool = plugin.get("enabled", false)
	_set_title(name)
	_set_status("%s  ·  number %d of %d in the load order" % ["On" if on else "Off", place, _plugins.size()])
	var toggle := _row("Turn off" if on else "Turn on", "The game skips this plugin" if on else "The game loads this plugin")
	toggle.set_meta("key", "toggle")
	toggle.pressed.connect(_toggle_plugin.bind(plugin))
	var up := _row("Load earlier", "Move up. Plugins below it can override it")
	up.set_meta("key", "up")
	up.pressed.connect(_move_plugin.bind(plugin, -1))
	var down := _row("Load later", "Move down. It overrides the plugins above it")
	down.set_meta("key", "down")
	down.pressed.connect(_move_plugin.bind(plugin, 1))


func _toggle_plugin(plugin: Dictionary) -> void:
	if _busy:
		return
	_remember_focus()
	var result: Dictionary = await _call(["plugins", "toggle", "--game", _game_id(), "--plugin", str(plugin.get("name", ""))], "Saving the plugin list…")
	_after_change(result, "%s is %s." % [plugin.get("name", "Plugin"), "off" if plugin.get("enabled", false) else "on"])


func _move_plugin(plugin: Dictionary, by: int) -> void:
	if _busy:
		return
	_remember_focus()
	var result: Dictionary = await _call(["plugins", "move", "--game", _game_id(), "--plugin", str(plugin.get("name", "")), "--by", str(by)], "Saving the load order…")
	if _take_state(result):
		_paint()
	else:
		_paint()
		_set_status(str(result.get("error", "Could not move that plugin.")), true)


# --- downloads -------------------------------------------------------------------------

func _paint_downloads() -> void:
	_set_title("Downloads")
	var game := _game()
	if game.is_empty():
		_note("Choose a game first.")
		return
	var signed: bool = _account.get("signed_in", false)
	var search := _row("Search Nexus Mods", "Find mods for %s by name" % game.get("name", "this game"))
	search.set_meta("key", "search")
	search.pressed.connect(_show.bind("search"))
	var site := _row("Browse the Nexus Mods website", "Press Mod Manager Download on any file there and it arrives here")
	site.set_meta("key", "site")
	site.pressed.connect(_open_url.bind("https://www.nexusmods.com/%s" % game.get("nexus_domain", "")))
	var who := str(_account.get("name", "")).strip_edges()
	var account_line := "Sign in to download"
	if signed:
		account_line = "%s  ·  %s" % [who if who != "" else "Signed in", "Premium" if _account.get("premium", false) else "Free account"]
	elif not _account_known:
		account_line = "Checking…"
	var account := _row("Nexus account", account_line, "", signed)
	account.set_meta("key", "account")
	account.pressed.connect(_show.bind("account"))
	if _link_handler == "unknown":
		_link_handler = _detect_link_handler()
	if _link_handler == "other":
		var catcher := _row("Catch Nexus download links", "Another app, or none, opens them now. Make it this one")
		catcher.set_meta("key", "handler")
		catcher.pressed.connect(_claim_links)
	var check := _row("Check the incoming folder", _short(_incoming))
	check.set_meta("key", "check")
	check.set_meta("details", "Put a .zip, .7z, or .rar in\n%s" % _incoming)
	check.pressed.connect(_check_incoming)

	if _pending.is_empty() and _failed.is_empty():
		_set_status("Nothing is waiting. New downloads install on their own unless they have options to pick." if signed else "Sign in to your Nexus account to download mods.")
		return
	_set_status("%d download%s need%s you." % [_pending.size() + _failed.size(), "" if _pending.size() + _failed.size() == 1 else "s", "s" if _pending.size() + _failed.size() == 1 else ""])
	_heading("Waiting")
	for item in _pending:
		var row := _row(str(item.get("module_name", item.get("name", "Archive"))), "Has options to choose  ·  %s" % _bytes(float(item.get("size", 0))), "SETUP", true)
		row.set_meta("key", str(item.get("path", "")))
		row.set_meta("item", item)
		row.set_meta("details", str(item.get("name", "")))
		row.pressed.connect(_open_wizard.bind(item))
	for item in _failed:
		var path := _incoming.path_join(str(item.get("name", "")))
		var broken := {"name": item.get("name", ""), "path": path, "error": item.get("error", "")}
		var row := _row(str(item.get("name", "Archive")), str(item.get("error", "Could not be installed")), "ERROR")
		row.set_meta("key", path)
		row.set_meta("item", broken)
		row.set_meta("details", str(item.get("error", "")))
		row.pressed.connect(_open_download.bind(broken))


func _check_incoming() -> void:
	if _busy:
		return
	_focus_key = "check"
	var result := await _sync("downloads")
	if result.get("ok", false) and (result.get("installed", []) as Array).is_empty() and _pending.is_empty() and _failed.is_empty():
		_set_status("Nothing new in the incoming folder.")


func _open_download(item: Dictionary) -> void:
	_target = item
	_show("download")


func _paint_download() -> void:
	_set_title(str(_target.get("name", "Download")))
	_set_status(str(_target.get("error", "")), true)
	var again := _row("Try again", "Install it once more")
	again.pressed.connect(_check_incoming)
	var discard := _row("Discard", "Delete this download")
	discard.pressed.connect(_ask_discard.bind(_target))


func _ask_discard(item: Dictionary) -> void:
	if str(item.get("path", "")) == "":
		return
	_confirm = {
		"back": "downloads",
		"title": "Discard %s?" % item.get("module_name", item.get("name", "this download")),
		"body": "The downloaded archive is deleted. You can download it again later.",
		"yes": "Discard",
		"action": _discard.bind(item),
	}
	_show("confirm")


func _discard(item: Dictionary) -> void:
	var result: Dictionary = await _call(["discard", "--game", _game_id(), "--from", str(item.get("path", ""))], "Deleting…")
	if not result.get("ok", false):
		_show("downloads")
		_set_status(str(result.get("error", "Could not delete that download.")), true)
		return
	await _sync("downloads", "Discarded %s." % item.get("name", "the download"))


# --- confirm ---------------------------------------------------------------------------

func _paint_confirm() -> void:
	_set_title(str(_confirm.get("title", "Are you sure?")))
	_set_status(str(_confirm.get("body", "")))
	var no := _row("Keep it", "")
	no.pressed.connect(_back)
	var yes := _row(str(_confirm.get("yes", "Yes")), "", "", false, BAD)
	yes.pressed.connect(_confirmed)


func _confirmed() -> void:
	if _busy:
		return
	var action: Callable = _confirm.get("action", Callable())
	if action.is_valid():
		await action.call()


# --- nexus search ------------------------------------------------------------------------

func _paint_search() -> void:
	_set_title("Search Nexus Mods")
	var split := HBoxContainer.new()
	split.size_flags_horizontal = SIZE_EXPAND_FILL
	split.add_theme_constant_override("separation", 20)
	_content.add_child(split)
	var left := VBoxContainer.new()
	left.size_flags_horizontal = SIZE_EXPAND_FILL
	left.add_theme_constant_override("separation", 10)
	split.add_child(left)
	var line := _line_edit("Mod name", false)
	line.text = _nexus_query
	line.set_meta("key", "query")
	line.text_submitted.connect(_submit_query)
	left.add_child(line)
	_rows.append(line)
	var go := _row("Search", "Leave the name empty to see the most popular mods", "", false, TEXT, left)
	go.set_meta("key", "go")
	go.pressed.connect(_submit_line.bind(line))
	for hit in _nexus_hits:
		var facts := "%s  ·  %s downloads  ·  %s endorsements" % [
			str(hit.get("author", "")).strip_edges() if str(hit.get("author", "")).strip_edges() != "" else "Unknown author",
			_compact(int(hit.get("downloads", 0))), _compact(int(hit.get("endorsements", 0))),
		]
		var row := _row(str(hit.get("name", "Mod")), facts, "", false, TEXT, left)
		row.set_meta("key", "mod-%d" % int(hit.get("mod_id", 0)))
		row.set_meta("details", str(hit.get("summary", "")))
		row.focus_entered.connect(_describe_hit.bind(hit))
		row.pressed.connect(_open_files.bind(hit))
	_side_panel(split, "Type a name and press Search.\n\nAdult mods are left out of these results.")
	if _nexus_hits.is_empty():
		_set_status("Highlight the name box to type. Steam + X also opens the keyboard.")
	else:
		_set_status("%d mods. Press A on one to see its files." % _nexus_hits.size())
		if _focus_key == "" or _focus_key == "query" or _focus_key == "go":
			_focus_key = "mod-%d" % int(_nexus_hits[0].get("mod_id", 0))


func _side_panel(parent: Control, text: String) -> void:
	var side := PanelContainer.new()
	side.custom_minimum_size = Vector2(360, 440)
	side.add_theme_stylebox_override("panel", _panel(Color(CARD, 0.86), 0, 4, 20))
	parent.add_child(side)
	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 14)
	side.add_child(box)
	_side_image = TextureRect.new()
	_side_image.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	_side_image.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_CENTERED
	_side_image.custom_minimum_size = Vector2(0, 190)
	_side_image.visible = false
	box.add_child(_side_image)
	_side_text = _label(text, 19, TEXT)
	_side_text.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(_side_text)


func _describe_hit(hit: Dictionary) -> void:
	if _side_text == null:
		return
	var summary := str(hit.get("summary", "")).strip_edges()
	_side_text.text = summary if summary != "" else str(hit.get("name", ""))
	_show_picture(str(hit.get("thumbnail", "")))


func _show_picture(source: String) -> void:
	if _side_image == null:
		return
	_thumb_wanted = source
	if _thumbs.has(source):
		_side_image.texture = _thumbs[source]
		_side_image.visible = true
		return
	_side_image.visible = false
	if source.begins_with("https://"):
		_http.cancel_request()
		_http.set_meta("url", source)
		_http.request(source)
	elif source != "" and FileAccess.file_exists(source):
		var image := Image.load_from_file(source)
		if image != null and not image.is_empty():
			_thumbs[source] = ImageTexture.create_from_image(image)
			_show_picture(source)


func _on_thumbnail(result: int, code: int, _headers: PackedStringArray, body: PackedByteArray) -> void:
	var source := str(_http.get_meta("url", ""))
	if result != HTTPRequest.RESULT_SUCCESS or code != 200 or body.size() < 12:
		return
	var image := Image.new()
	var error := ERR_FILE_UNRECOGNIZED
	if body[0] == 0x89 and body[1] == 0x50:
		error = image.load_png_from_buffer(body)
	elif body[0] == 0xff and body[1] == 0xd8:
		error = image.load_jpg_from_buffer(body)
	elif body.slice(8, 12).get_string_from_ascii() == "WEBP":
		error = image.load_webp_from_buffer(body)
	if error != OK or image.is_empty():
		return
	_thumbs[source] = ImageTexture.create_from_image(image)
	if source == _thumb_wanted and _side_image != null:
		_side_image.texture = _thumbs[source]
		_side_image.visible = true


func _submit_line(line: LineEdit) -> void:
	_submit_query(line.text)


func _submit_query(text: String) -> void:
	if _busy:
		return
	_nexus_query = text.strip_edges()
	await _search()


func _search() -> void:
	var result: Dictionary = await _call(["nexus", "search", "--game", _game_id(), "--query", _nexus_query], "Searching Nexus Mods…")
	_nexus_hits = result.get("mods", []) if result.get("ok", false) else []
	_focus_key = ""
	_show("search")
	if not result.get("ok", false):
		_set_status(str(result.get("error", "Search failed.")), true)
	elif _nexus_hits.is_empty():
		_set_status("No mods matched \"%s\"." % _nexus_query)


func _open_files(hit: Dictionary) -> void:
	if _busy:
		return
	if not _account.get("signed_in", false):
		_show("account")
		_set_status("Sign in first. Nexus only lists a mod's files for signed-in accounts.")
		return
	var result: Dictionary = await _call(["nexus", "files", "--game", _game_id(), "--mod", str(int(hit.get("mod_id", 0)))], "Loading files…")
	if not result.get("ok", false):
		_show("search")
		_set_status(str(result.get("error", "Could not list files.")), true)
		return
	_nexus_mod = hit
	_nexus_files = []
	for file in result.get("files", []):
		if str(file.get("category", "")) not in ["DELETED", "ARCHIVED", "REMOVED"]:
			_nexus_files.append(file)
	_show("files")


func _paint_files() -> void:
	_set_title(str(_nexus_mod.get("name", "Files")))
	var premium: bool = _account.get("premium", false)
	var page := _row("Open the mod page", "Description, pictures, and requirements on the Nexus website")
	page.set_meta("key", "page")
	page.pressed.connect(_open_mod_page.bind({"domain": _game().get("nexus_domain", ""), "mod_id": _nexus_mod.get("mod_id", 0)}, false))
	if _nexus_files.is_empty():
		_set_status("This mod has no files to download.")
		return
	if premium:
		_set_status("Press A on a file to download and install it.")
	else:
		_set_status("Press A on a file. Its download page opens in the browser, and the file arrives here when you press Slow Download.")
	for file in _nexus_files:
		var category := str(file.get("category", "FILE"))
		var facts: Array[String] = []
		if str(file.get("version", "")) != "":
			facts.append("v" + str(file.get("version", "")).trim_prefix("v"))
		if int(file.get("size_kb", 0)) > 0:
			facts.append(_bytes(float(file.get("size_kb", 0)) * 1024.0))
		facts.append(str(file.get("file_name", "")))
		var badge: String = {"MAIN": "MAIN", "UPDATE": "UPDATE", "OPTIONAL": "EXTRA", "OLD_VERSION": "OLD", "MISCELLANEOUS": "MISC"}.get(category, "FILE")
		var row := _row(str(file.get("name", file.get("file_name", "File"))), "  ·  ".join(facts), badge, category == "MAIN")
		row.set_meta("key", "file-%d" % int(file.get("file_id", 0)))
		row.set_meta("details", str(file.get("file_name", "")))
		row.pressed.connect(_download.bind(file))
	if _focus_key == "":
		_focus_key = "file-%d" % int(_nexus_files[0].get("file_id", 0))


func _download(file: Dictionary) -> void:
	if _busy:
		return
	_remember_focus()
	var mod_id := int(_nexus_mod.get("mod_id", 0))
	var file_id := int(file.get("file_id", 0))
	if not _account.get("premium", false):
		_open_url("https://www.nexusmods.com/%s/mods/%d?tab=files&file_id=%d&nmm=1" % [_game().get("nexus_domain", ""), mod_id, file_id])
		_set_status("Opened the download page. Press Slow Download there. This app takes over as soon as the browser hands the file across.")
		return
	var progress := _scratch("download-progress.json")
	var result: Dictionary = await _call([
		"nexus", "download", "--game", _game_id(), "--mod", str(mod_id), "--file", str(file_id), "--progress", progress,
	], "Downloading %s…" % file.get("name", "the file"), progress)
	if not result.get("ok", false):
		_paint()
		_set_status(str(result.get("error", "The download failed.")), true)
		return
	await _finish_download(result)


## A new archive is in incoming. Install it, or open its options.
func _finish_download(saved: Dictionary) -> void:
	var path := str(saved.get("path", ""))
	var label := str(saved.get("mod_name", "")) if str(saved.get("mod_name", "")) != "" else str(saved.get("name", "the mod"))
	var result := await _sync("mods")
	if not result.get("ok", false):
		return
	for item in _pending:
		if str(item.get("path", "")) == path:
			await _open_wizard(item)
			return
	for item in _failed:
		if _incoming.path_join(str(item.get("name", ""))) == path:
			_show("downloads")
			_set_status("%s could not be installed: %s" % [label, item.get("error", "")], true)
			return
	for mod in result.get("installed", []):
		_focus_key = str(mod.get("id", ""))
	_paint()
	_set_status("Installed %s. It is on." % label)


# --- nxm links ---------------------------------------------------------------------------

func _poll_links() -> void:
	if _busy or _view == "wizard" or _view == "confirm" or not _core.available():
		return
	var queue := _queue_dir()
	if not DirAccess.dir_exists_absolute(queue):
		return
	var files := DirAccess.get_files_at(queue)
	files.sort()
	for file in files:
		if file.ends_with(".nxm"):
			await _handle_link(queue.path_join(file))
			return


func _handle_link(file: String) -> void:
	# Bring the window forward so the download does not happen out of sight.
	DisplayServer.window_request_attention()
	DisplayServer.window_move_to_foreground()
	var progress := _scratch("download-progress.json")
	var args := ["nexus", "nxm", "--from", file, "--progress", progress]
	if _game_id() != "":
		args.append_array(["--prefer", _game_id()])
	var result: Dictionary = await _call(args, "Downloading from Nexus Mods…", progress)
	# The link is single use. Keeping a failed one would retry it forever.
	DirAccess.remove_absolute(file)
	if not result.get("ok", false):
		var error := str(result.get("error", "That download did not work."))
		if error.begins_with("Sign in"):
			_show("account")
		else:
			_paint()
		_set_status(error, true)
		return
	var game_id := str(result.get("game", ""))
	for index in _games.size():
		if str(_games[index].get("id", "")) == game_id and index != _selected:
			_selected = index
			_mods = []
			_plugins = []
			_save_setting("last_game", game_id)
	await _finish_download(result)


func _links_from_arguments() -> PackedStringArray:
	var links := PackedStringArray()
	var arguments := OS.get_cmdline_args()
	arguments.append_array(OS.get_cmdline_user_args())
	for argument in arguments:
		if argument.to_lower().begins_with("nxm://") and not links.has(argument):
			links.append(argument)
	if links.is_empty():
		return links
	DirAccess.make_dir_recursive_absolute(_queue_dir())
	for index in links.size():
		var path := _queue_dir().path_join("%d-%d.nxm" % [Time.get_ticks_usec() + int(Time.get_unix_time_from_system()) * 1000000, index])
		var file := FileAccess.open(path, FileAccess.WRITE)
		if file != null:
			file.store_string(links[index])
			file.close()
	return links


func _state_dir() -> String:
	var base := OS.get_environment("XDG_STATE_HOME")
	if base == "":
		base = OS.get_environment("HOME").path_join(".local/state")
	return base.path_join("vortex-steamos")


func _queue_dir() -> String:
	return _state_dir().path_join("links")


func _pid_path() -> String:
	return _state_dir().path_join("ui.pid")


func _write_pid() -> void:
	DirAccess.make_dir_recursive_absolute(_state_dir())
	var file := FileAccess.open(_pid_path(), FileAccess.WRITE)
	if file != null:
		file.store_string(str(OS.get_process_id()))


func _another_instance_running() -> bool:
	var path := _pid_path()
	if not FileAccess.file_exists(path):
		return false
	var pid := int(FileAccess.get_file_as_string(path).strip_edges())
	if pid <= 0 or pid == OS.get_process_id():
		return false
	if OS.get_name() == "Linux":
		return DirAccess.dir_exists_absolute("/proc/%d" % pid)
	return OS.is_process_running(pid)


func _detect_link_handler() -> String:
	if OS.get_name() != "Linux":
		return "unsupported"
	var entry := OS.get_environment("HOME").path_join(".local/share/applications").path_join(DESKTOP_ID)
	if not FileAccess.file_exists(entry):
		return "unsupported"
	var output: Array = []
	if OS.execute("xdg-mime", PackedStringArray(["query", "default", "x-scheme-handler/nxm"]), output) != 0:
		return "unsupported"
	return "ours" if "".join(output).strip_edges() == DESKTOP_ID else "other"


func _claim_links() -> void:
	var code := OS.execute("xdg-mime", PackedStringArray(["default", DESKTOP_ID, "x-scheme-handler/nxm"]))
	_link_handler = _detect_link_handler()
	_focus_key = "site"
	_paint()
	if code == 0 and _link_handler == "ours":
		_set_status("Done. Mod Manager Download buttons on Nexus now send files here.")
	else:
		_set_status("Could not change the handler. Run the installer again from a terminal.", true)


func _open_mod_page(nexus: Dictionary, files_tab: bool) -> void:
	var url := "https://www.nexusmods.com/%s/mods/%d" % [nexus.get("domain", ""), int(nexus.get("mod_id", 0))]
	_open_url(url + ("?tab=files" if files_tab else ""))


func _open_url(url: String) -> void:
	# In Game Mode there is no desktop browser on screen. Steam's own can show the page.
	var target := "steam://openurl/" + url if _game_mode() else url
	if OS.shell_open(target) == OK:
		_set_status("Opened %s in the browser." % url.get_slice("?", 0).trim_prefix("https://"))
	else:
		_set_status("Could not open the browser. The address is %s" % url, true)


# --- account -----------------------------------------------------------------------------

func _paint_account() -> void:
	_set_title("Nexus account")
	if _account.get("signed_in", false):
		var who := str(_account.get("name", "")).strip_edges()
		if _account.get("premium", false):
			_set_status("Signed in as %s. Premium: files download right here." % (who if who != "" else "your account"))
		else:
			_set_status("Signed in as %s. With a free account, press Mod Manager Download on the website and the file arrives here." % (who if who != "" else "your account"))
		var out := _row("Sign out", "Forget the key saved on this device")
		out.set_meta("key", "out")
		out.pressed.connect(_sign_out)
		return
	_set_status("Nexus signs this app in with a personal API key. Get one from your account page, then paste it below.")
	var get_key := _row("1.  Get your key", "Opens your Nexus account's API keys page. Copy the Personal API Key at the bottom")
	get_key.set_meta("key", "get")
	get_key.pressed.connect(_open_url.bind(API_KEY_PAGE))
	var line := _line_edit("2.  Paste the key here", true)
	line.set_meta("key", "line")
	line.text_submitted.connect(_submit_key)
	_content.add_child(line)
	_rows.append(line)
	var save := _row("3.  Sign in", "The key is checked with Nexus and saved for your user only")
	save.set_meta("key", "save")
	save.pressed.connect(_submit_key_line.bind(line))
	_note("On a Steam Deck, Steam + X opens the keyboard and its paste key inserts what you copied.")


func _submit_key_line(line: LineEdit) -> void:
	var secret := line.text
	line.text = ""
	_submit_key(secret)


func _submit_key(secret: String) -> void:
	if _busy:
		return
	secret = secret.strip_edges()
	if secret == "":
		_set_status("Paste your key first.", true)
		return
	# The key goes to the core in a private file, never on a command line.
	var path := _scratch("nexus-key.txt")
	var file := FileAccess.open(path, FileAccess.WRITE)
	if file == null:
		_set_status("Could not hand the key to the core.", true)
		return
	file.close()
	FileAccess.set_unix_permissions(path, 384)
	file = FileAccess.open(path, FileAccess.WRITE)
	file.store_string(secret)
	file.close()
	secret = ""
	var result: Dictionary = await _call(["nexus", "store-key", "--from", path], "Checking the key with Nexus…")
	if FileAccess.file_exists(path):
		DirAccess.remove_absolute(path)
	if not result.get("ok", false):
		_paint()
		_set_status(str(result.get("error", "Nexus rejected that key.")), true)
		return
	_account = result
	_account_known = true
	_focus_key = "search"
	_show("downloads")
	_set_status("Signed in as %s." % (str(result.get("name", "")) if str(result.get("name", "")) != "" else "your Nexus account"))


func _sign_out() -> void:
	if _busy:
		return
	var result: Dictionary = await _call(["nexus", "clear-key"], "Signing out…")
	if not result.get("ok", false):
		_set_status(str(result.get("error", "Could not sign out.")), true)
		return
	_account = {"ok": true, "signed_in": false, "name": "", "premium": false}
	_show("account")
	if result.get("env_override", false):
		_set_status("Signed out. VORTEX_NEXUS_KEY is still set in the environment, so that key stays in use.")


# --- installer wizard ---------------------------------------------------------------------

func _open_wizard(item: Dictionary) -> void:
	if _busy:
		return
	_wizard_path = str(item.get("path", ""))
	_wizard_name = str(item.get("module_name", item.get("name", "Installer")))
	_wizard_step = 0
	_wizard_choices = []
	_wizard_plan = {}
	await _reload_wizard()


func _reload_wizard() -> void:
	var args := ["fomod", "--game", _game_id(), "--from", _wizard_path]
	if not _wizard_choices.is_empty():
		var choices := _write_choices()
		if choices == "":
			_set_status("Could not save these choices.", true)
			return
		args.append_array(["--choices", choices])
	var result: Dictionary = await _call(args, "Reading the installer…")
	if not result.get("ok", false):
		_show("downloads")
		_set_status(str(result.get("error", "Could not read this installer.")), true)
		return
	_wizard_plan = result
	_view = "wizard"
	_paint()


func _visible_steps() -> Array:
	var steps: Array = []
	for step in _wizard_plan.get("steps", []):
		if step.get("visible", false):
			steps.append(step)
	return steps


func _paint_wizard() -> void:
	_set_title(_wizard_name)
	var steps := _visible_steps()
	if steps.is_empty():
		_set_status("Nothing to choose for the mods you have. Install it as it is.")
		var only := _row("Install", "")
		only.pressed.connect(_wizard_install)
		return
	_wizard_step = clampi(_wizard_step, 0, steps.size() - 1)
	var step: Dictionary = steps[_wizard_step]
	_set_status("Step %d of %d  ·  %s" % [_wizard_step + 1, steps.size(), step.get("name", "Options")])
	var split := HBoxContainer.new()
	split.size_flags_horizontal = SIZE_EXPAND_FILL
	split.add_theme_constant_override("separation", 20)
	_content.add_child(split)
	var left := VBoxContainer.new()
	left.size_flags_horizontal = SIZE_EXPAND_FILL
	left.add_theme_constant_override("separation", 10)
	split.add_child(left)
	for group in step.get("groups", []):
		var heading := "%s   ·   %s" % [group.get("name", "Options"), group.get("hint", "")]
		left.add_child(_label(heading, 17, MUTED))
		if not group.get("valid", true):
			left.add_child(_label(str(group.get("problem", "Pick an option.")), 17, BAD))
		for option in group.get("plugins", []):
			var kind := str(option.get("kind", ""))
			var chosen: bool = option.get("selected", false)
			var note := {"Required": "Required", "Recommended": "Recommended", "NotUsable": "Not available with your setup"}.get(kind, "") as String
			var row := _row(str(option.get("name", "Option")), note, "✓" if chosen else " ", chosen, TEXT, left)
			row.disabled = option.get("locked", false)
			row.focus_mode = FOCUS_ALL
			row.set_meta("key", "%s/%s" % [group.get("name", ""), option.get("name", "")])
			row.set_meta("item", option)
			row.set_meta("details", str(option.get("description", "")))
			row.focus_entered.connect(_describe_option.bind(option))
			row.pressed.connect(_wizard_pick.bind(str(step.get("name", "")), group, str(option.get("name", ""))))
	var last := _wizard_step >= steps.size() - 1
	var next := _row("Install" if last else "Next", "", "", false, ACCENT, left)
	next.set_meta("key", "next")
	next.pressed.connect(_wizard_advance)
	_side_panel(split, "Highlight an option to read about it.")


func _describe_option(option: Dictionary) -> void:
	if _side_text == null:
		return
	var text := str(option.get("description", "")).strip_edges()
	_side_text.text = text if text != "" else str(option.get("name", ""))
	var image = option.get("image")
	_show_picture("" if image == null else str(image))


func _wizard_pick(step_name: String, group: Dictionary, option_name: String) -> void:
	if _busy:
		return
	var group_name := str(group.get("name", ""))
	var names := _choice_names(step_name, group)
	match str(group.get("select", "")):
		"SelectExactlyOne":
			names = [option_name]
		"SelectAtMostOne":
			names = [] if names == [option_name] else [option_name]
		_:
			if names.has(option_name):
				names.erase(option_name)
			else:
				names.append(option_name)
	var stored := false
	for choice in _wizard_choices:
		if str(choice.get("step", "")) == step_name and str(choice.get("group", "")) == group_name:
			choice["plugins"] = names
			stored = true
	if not stored:
		_wizard_choices.append({"step": step_name, "group": group_name, "plugins": names})
	_focus_key = "%s/%s" % [group_name, option_name]
	await _reload_wizard()


func _choice_names(step_name: String, group: Dictionary) -> Array:
	for choice in _wizard_choices:
		if str(choice.get("step", "")) == step_name and str(choice.get("group", "")) == str(group.get("name", "")):
			return (choice.get("plugins", []) as Array).duplicate()
	var names: Array = []
	for option in group.get("plugins", []):
		if option.get("selected", false) and str(option.get("kind", "")) != "Required":
			names.append(str(option.get("name", "")))
	return names


func _write_choices() -> String:
	var path := _scratch("fomod-choices.json")
	var file := FileAccess.open(path, FileAccess.WRITE)
	if file == null:
		return ""
	file.store_string(JSON.stringify(_wizard_choices))
	file.close()
	return path


func _wizard_advance() -> void:
	if _busy:
		return
	var steps := _visible_steps()
	if steps.is_empty():
		await _wizard_install()
		return
	var step: Dictionary = steps[clampi(_wizard_step, 0, steps.size() - 1)]
	if not step.get("valid", true):
		_set_status("%s still needs a choice." % step.get("name", "This step"), true)
		return
	if _wizard_step < steps.size() - 1:
		_wizard_step += 1
		_focus_key = ""
		_paint()
	else:
		await _wizard_install()


func _wizard_back() -> void:
	if _wizard_step > 0:
		_wizard_step -= 1
		_focus_key = ""
		_paint()
	else:
		_show("downloads")
		_set_status("%s is still waiting here. Press X on it to discard it." % _wizard_name)


func _wizard_install() -> void:
	if _busy:
		return
	var args := ["install", "--game", _game_id(), "--from", _wizard_path]
	if not _wizard_choices.is_empty():
		var choices := _write_choices()
		if choices == "":
			_set_status("Could not save these choices.", true)
			return
		args.append_array(["--choices", choices])
	var result: Dictionary = await _call(args, "Installing %s…" % _wizard_name)
	if not result.get("ok", false):
		_paint()
		_set_status(str(result.get("error", "Could not install that archive.")), true)
		return
	_take_state(result)
	var installed: Array = result.get("installed", [])
	_pending = _pending.filter(func(item): return str(item.get("path", "")) != _wizard_path)
	_focus_key = str(installed[0].get("id", "")) if not installed.is_empty() else ""
	_view = "mods"
	_paint()
	_set_status("Installed %s. It is on." % (_name_list(installed) if not installed.is_empty() else _wizard_name))


# --- settings and files ---------------------------------------------------------------------

func _settings() -> Dictionary:
	if not FileAccess.file_exists("user://settings.json"):
		return {}
	var parsed = JSON.parse_string(FileAccess.get_file_as_string("user://settings.json"))
	return parsed if typeof(parsed) == TYPE_DICTIONARY else {}


func _save_setting(key: String, value: Variant) -> void:
	var settings := _settings()
	settings[key] = value
	var file := FileAccess.open("user://settings.json", FileAccess.WRITE)
	if file != null:
		file.store_string(JSON.stringify(settings))


func _scratch(name: String) -> String:
	var dir := OS.get_cache_dir().path_join("vortex-steamos")
	DirAccess.make_dir_recursive_absolute(dir)
	return dir.path_join(name)


# --- widgets ----------------------------------------------------------------------------------

## One focusable line in a list: a badge, a title, and a quieter second line.
func _row(title: String, subtitle := "", badge := "", marked := false, title_color := TEXT, parent: Control = null) -> Button:
	var button := Button.new()
	button.focus_mode = FOCUS_ALL
	button.custom_minimum_size = Vector2(0, 74 if subtitle != "" else 58)
	button.clip_contents = true
	var focus := _focus_panel(4, 0)
	button.add_theme_stylebox_override("normal", _panel(Color(CARD, 0.86), 0, 4, 0))
	button.add_theme_stylebox_override("disabled", _panel(Color(CARD, 0.45), 0, 4, 0))
	# A mouse resting on a row must not look like the controller's highlight.
	button.add_theme_stylebox_override("hover", _panel(CARD.lightened(0.06), 0, 4, 0))
	for state in ["pressed", "focus"]:
		button.add_theme_stylebox_override(state, focus)
	var margin := _margin(20, 0, 20, 0)
	margin.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	margin.mouse_filter = MOUSE_FILTER_IGNORE
	button.add_child(margin)
	var line := HBoxContainer.new()
	line.mouse_filter = MOUSE_FILTER_IGNORE
	line.add_theme_constant_override("separation", 16)
	margin.add_child(line)
	if badge != "":
		var tag := _label(badge, 14, TEXT if marked else (BAD if badge == "ERROR" else MUTED))
		tag.size_flags_horizontal = SIZE_SHRINK_BEGIN
		tag.size_flags_vertical = SIZE_SHRINK_CENTER
		tag.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
		tag.custom_minimum_size = Vector2(30 if badge.length() <= 1 else 74, 26)
		tag.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
		# A filled pill for "on", an outline for everything else.
		var pill := _panel(ACCENT if marked else Color(0, 0, 0, 0), 0, 13, 0)
		if not marked:
			pill.set_border_width_all(1)
			pill.border_color = Color(MUTED, 0.5)
		tag.add_theme_stylebox_override("normal", pill)
		line.add_child(tag)
	var texts := VBoxContainer.new()
	texts.mouse_filter = MOUSE_FILTER_IGNORE
	texts.size_flags_horizontal = SIZE_EXPAND_FILL
	texts.alignment = BoxContainer.ALIGNMENT_CENTER
	texts.add_theme_constant_override("separation", 2)
	line.add_child(texts)
	var head := _label(title, 22, title_color)
	head.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	texts.add_child(head)
	if subtitle != "":
		var sub := _label(subtitle, 16, MUTED)
		sub.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
		texts.add_child(sub)
	(parent if parent != null else _content).add_child(button)
	_rows.append(button)
	return button


func _line_edit(placeholder: String, secret: bool) -> LineEdit:
	var line := LineEdit.new()
	line.secret = secret
	line.placeholder_text = placeholder
	line.virtual_keyboard_enabled = true
	line.custom_minimum_size = Vector2(0, 58)
	line.add_theme_font_size_override("font_size", 22)
	line.add_theme_color_override("font_color", TEXT)
	line.add_theme_color_override("font_placeholder_color", MUTED)
	line.add_theme_stylebox_override("normal", _panel(Color(CARD, 0.86), 0, 4, 16))
	line.add_theme_stylebox_override("focus", _focus_panel(4, 16))
	line.focus_entered.connect(_open_keyboard)
	return line


## Under Steam there is no keyboard until Steam is asked for its own.
func _open_keyboard() -> void:
	if _game_mode() and not _busy:
		OS.shell_open("steam://open/keyboard")


func _note(body: String) -> void:
	var label := _label(body, 20, TEXT)
	label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_content.add_child(label)


func _heading(text: String) -> void:
	_content.add_child(_gap(6))
	_content.add_child(_label(text.to_upper(), 15, MUTED))


func _label(text: String, size: int, color: Color) -> Label:
	var label := Label.new()
	label.text = text
	label.mouse_filter = MOUSE_FILTER_IGNORE
	label.size_flags_horizontal = SIZE_EXPAND_FILL
	label.add_theme_font_size_override("font_size", size)
	label.add_theme_color_override("font_color", color)
	return label


func _margin(left: int, top: int, right: int, bottom: int) -> MarginContainer:
	var margin := MarginContainer.new()
	margin.add_theme_constant_override("margin_left", left)
	margin.add_theme_constant_override("margin_top", top)
	margin.add_theme_constant_override("margin_right", right)
	margin.add_theme_constant_override("margin_bottom", bottom)
	return margin


func _panel(color: Color, border: int, radius: int, padding: int) -> StyleBoxFlat:
	var box := StyleBoxFlat.new()
	box.bg_color = color
	box.set_corner_radius_all(radius)
	box.set_content_margin_all(padding)
	box.border_width_left = border
	box.border_color = ACCENT
	return box


## The highlighted row: a lighter fill and a white outline, as in Big Picture.
## It has to read from across a room.
func _focus_panel(radius: int, padding: int) -> StyleBoxFlat:
	var box := _panel(CARD_ON, 0, radius, padding)
	box.set_border_width_all(3)
	box.border_color = TEXT
	return box


func _gap_wide(width: int) -> Control:
	var gap := Control.new()
	gap.custom_minimum_size = Vector2(width, 0)
	return gap


## Room around `inner`, so a focus outline at its edge is not clipped.
func _pad(inner: Control, by: int) -> MarginContainer:
	var margin := _margin(by, by, by, by)
	margin.add_child(inner)
	return margin


func _gap(height: int) -> Control:
	var gap := Control.new()
	gap.custom_minimum_size = Vector2(0, height)
	return gap


func _short(path: String) -> String:
	var home := OS.get_environment("HOME")
	if home != "" and path.begins_with(home):
		path = "~" + path.substr(home.length())
	if path.length() <= 64:
		return path
	return path.substr(0, 24) + "…" + path.substr(path.length() - 38)


func _compact(value: int) -> String:
	if value >= 1000000:
		return "%.1fM" % (float(value) / 1000000.0)
	if value >= 1000:
		return "%dk" % int(round(float(value) / 1000.0))
	return str(value)


func _bytes(value: float) -> String:
	if value >= 1073741824.0:
		return "%.1f GB" % (value / 1073741824.0)
	if value >= 1048576.0:
		return "%.0f MB" % (value / 1048576.0)
	if value >= 1024.0:
		return "%.0f KB" % (value / 1024.0)
	return "%d bytes" % int(value)
