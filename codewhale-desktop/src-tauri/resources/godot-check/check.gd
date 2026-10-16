extends SceneTree

# Headless shader compile check used by the ai-drive MCP server (godot_check) and by hand:
#   godot --headless --path <this folder> --script res://check.gd -- <file.gdshader>
# The dummy renderer still runs the shader parser, so syntax and type errors are printed
# as "SHADER ERROR: ..." followed by "ERROR: Shader compilation failed.".
func _init() -> void:
	var args := OS.get_cmdline_user_args()
	if args.is_empty():
		printerr("usage: -- <path.gdshader>")
		quit(2)
		return
	var path: String = args[0]
	var f := FileAccess.open(path, FileAccess.READ)
	if f == null:
		printerr("CANNOT_OPEN %s (%s)" % [path, error_string(FileAccess.get_open_error())])
		quit(2)
		return
	var sh := Shader.new()
	sh.code = f.get_as_text()
	var uniforms := sh.get_shader_uniform_list()
	print("uniforms=%d" % uniforms.size())
	for u in uniforms:
		print("uniform %s" % u.get("name", "?"))
	print("RESULT: LOADED")
	quit(0)
