extends Node3D

# Builds the water demo scene in code (camera, sun, sky, sea floor, a few half-submerged
# boxes and the water plane with water_material.tres). With `-- --shot <path>` it renders
# 30 frames, saves the viewport to <path> and quits, so the whole thing runs under Xvfb:
#   godot --rendering-driver opengl3 --path Godot/WaterDemo -- --shot /tmp/water.png

const WATER_MATERIAL := "res://water_material.tres"
const SHOT_FRAME := 30

var _frames := 0
var _shot_path := ""


func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	var i := args.find("--shot")
	if i >= 0 and i + 1 < args.size():
		_shot_path = args[i + 1]

	var cam := Camera3D.new()
	cam.position = Vector3(0.0, 6.0, 12.0)
	add_child(cam)
	cam.look_at(Vector3(0.0, 0.0, 0.0))
	cam.current = true

	var sun := DirectionalLight3D.new()
	sun.rotation_degrees = Vector3(-50.0, 30.0, 0.0)
	sun.light_energy = 1.2
	sun.shadow_enabled = true
	add_child(sun)

	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color(0.55, 0.7, 0.9)
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color(0.6, 0.7, 0.8)
	env.ambient_light_energy = 0.6
	var world_env := WorldEnvironment.new()
	world_env.environment = env
	add_child(world_env)

	var sea_floor := MeshInstance3D.new()
	var floor_mesh := PlaneMesh.new()
	floor_mesh.size = Vector2(40.0, 40.0)
	sea_floor.mesh = floor_mesh
	sea_floor.position.y = -3.0
	var floor_mat := StandardMaterial3D.new()
	floor_mat.albedo_color = Color(0.76, 0.7, 0.5)
	sea_floor.material_override = floor_mat
	add_child(sea_floor)

	for p in [Vector3(-4.0, -0.5, 0.0), Vector3(2.0, 0.2, -3.0), Vector3(5.0, -1.0, 4.0)]:
		var box := MeshInstance3D.new()
		box.mesh = BoxMesh.new()
		box.mesh.size = Vector3(2.0, 2.0, 2.0)
		box.position = p
		var box_mat := StandardMaterial3D.new()
		box_mat.albedo_color = Color(0.8, 0.35, 0.25)
		box.material_override = box_mat
		add_child(box)

	var water := MeshInstance3D.new()
	var water_mesh := PlaneMesh.new()
	water_mesh.size = Vector2(40.0, 40.0)
	water_mesh.subdivide_width = 160
	water_mesh.subdivide_depth = 160
	water.mesh = water_mesh
	water.material_override = load(WATER_MATERIAL)
	add_child(water)


func _process(_delta: float) -> void:
	_frames += 1
	if _shot_path.is_empty() or _frames != SHOT_FRAME:
		return
	await RenderingServer.frame_post_draw
	var img := get_viewport().get_texture().get_image()
	var err := img.save_png(_shot_path)
	if err == OK:
		print("SHOT_SAVED %s %dx%d" % [_shot_path, img.get_width(), img.get_height()])
	else:
		printerr("SHOT_FAILED %s (%s)" % [_shot_path, error_string(err)])
	get_tree().quit(0 if err == OK else 1)
