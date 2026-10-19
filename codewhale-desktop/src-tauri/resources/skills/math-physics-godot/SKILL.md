---
name: math-physics-godot
description: Физика и математика для Godot 4 (GDScript, gdshader) — пружина вместо lerp, Верле, RK4, вода Герстнера с пеной и френелем, плавучесть, подвеска на рейкастах. Use when: пружина, отдача, Verlet, RK4, волна, вода, Gerstner, плавучесть, buoyancy, подвеска, raycast vehicle, нормаль, SDF, Y-up.
invocation: model+user
---

# Физика и математика Godot 4

## Главное (вывод обрезан: нужное здесь и в конце)
1. Файлы — в КОРЕНЬ проекта (папка с `project.godot`, обычно рабочая): `res://water.gdshader` = `<рабочая папка>/water.gdshader`. Папки `project/`, `res/` не создавай.
2. Вода: `write` скелета из конца → `mcp_ai--drive_godot_check {"path":"<рабочая папка>/water.gdshader"}` → правка → снова `godot_check`.
3. В ошибке `(:N)` — номер строки, `Подсказка:`/`Hint:` — что делать. Исправь ровно строку N. Один `edit` — одно изменение.
4. `edit` не прошёл 2 раза → `read` файла, потом `write` всего файла с переписанной функцией.
5. «Готово» — только если последний `godot_check` вернул `ok: true`, затем `mcp_ai--drive_ledger_record`. Иначе назови оставшуюся ошибку.
6. Пиши коротко: 1–2 предложения перед вызовом, без пересказов и повторов.
7. Скрытый код: `retrieve_tool_result {"ref":"<ref из omitted>","mode":"lines","lines":"38-51"}` (пружина), `"53-76"` (RK4), `"78-107"` (плавучесть, подвеска).

## Оси и шейдеры
- Инструменты `mcp_ai--drive_*` уже в списке (нет — `tool_search {"query":"mcp_ai-drive_code_search"}`, один дефис);
  пример шейдера — `mcp_ai--drive_code_search {"query":"shader_type spatial water"}`,
  `ledger_record` — `{"task_id":"water","script_path":"<путь>","summary":"..."}`. `Web` не нужен.
- Godot: **Y вверх, -Z вперёд**; `Vector3.FORWARD == Vector3(0, 0, -1)`; `basis.z` — «назад», вперёд `-basis.z`.
  Из Blender (Z вверх): `Vector3(b.x, b.z, -b.y)`.
- Нормаль поля/SDF `f(x, y, z)` = `normalize(gradient)`: `math_eval {"mode":"gradient3d","expr":...}` → готовый `Vector3(...)`.
  Производные — через `math_eval`, не «в уме».
- GDScript: статическая типизация, API Godot 4, физика в `_physics_process(delta)`; `move_and_slide()` без
  аргументов, скорость в `velocity`.
- `.gdshader` — язык Godot, не GLSL: первая строка `shader_type spatial;`; встроенные `VERTEX`, `NORMAL`, `UV`,
  `TIME`, `ALBEDO`, `ROUGHNESS`, `VIEW`. Нет `gl_FragColor`, `gl_Position`, `texture2D`, `#version`, `main()`.
  Числа с точкой: `2.0 * x`, `pow(x, 2.0)`; `PI` встроена, не объявляй. `Shader type  not supported` = нет `shader_type spatial;`.
- Цвет — `ALBEDO`; `COLOR` в spatial только читается («Constants cannot be modified»). `UV` — `vec2`: `vec3(UV, 0.0)`,
  иначе «Invalid assignment of 'vec2' to 'vec3'». Глобальные переменные — только `uniform`/`const`/`varying`.

## Пружина с критическим затуханием (камера, отдача, UI) — вместо `lerp`
`lerp` каждый кадр зависит от FPS (или `lerp(a, b, 1.0 - exp(-rate * delta))`); пружина — нет и не перелетает
цель (`zeta = 1`). `omega` ≈ `2π / время_отклика_сек`.
```gdscript
static func spring_damp(current: Vector3, target: Vector3, velocity: Vector3, omega: float, dt: float) -> Array[Vector3]:
	var exp_term: float = exp(-omega * dt)
	var diff: Vector3 = current - target
	var temp: Vector3 = (velocity + omega * diff) * dt
	var new_pos: Vector3 = target + (diff + temp) * exp_term
	var new_vel: Vector3 = (velocity - omega * temp) * exp_term
	var out: Array[Vector3] = [new_pos, new_vel]
	return out
```
В `_process`: `var r := spring_damp(position, target.position, _vel, 12.0, delta); position = r[0]; _vel = r[1]`.

## Интегрирование: Верле и RK4
Явный Эйлер (`pos += vel * dt; vel += acc * dt`) накапливает энергию — не для пружин и орбит.
```gdscript
# Verlet: keeps the previous position; ropes, cloth, chains.
static func verlet_step(pos: Vector3, prev_pos: Vector3, accel: Vector3, dt: float) -> Array[Vector3]:
	var next: Vector3 = 2.0 * pos - prev_pos + accel * dt * dt
	var out: Array[Vector3] = [next, pos]
	return out

# RK4: accel.call(pos, vel) -> Vector3; drag ballistics, orbits.
static func rk4_step(pos: Vector3, vel: Vector3, accel: Callable, dt: float) -> Array[Vector3]:
	var a1: Vector3 = accel.call(pos, vel)
	var v2: Vector3 = vel + a1 * (dt * 0.5)
	var a2: Vector3 = accel.call(pos + vel * (dt * 0.5), v2)
	var v3: Vector3 = vel + a2 * (dt * 0.5)
	var a3: Vector3 = accel.call(pos + v2 * (dt * 0.5), v3)
	var v4: Vector3 = vel + a3 * dt
	var a4: Vector3 = accel.call(pos + v3 * dt, v4)
	var new_pos: Vector3 = pos + (vel + 2.0 * v2 + 2.0 * v3 + v4) * (dt / 6.0)
	var new_vel: Vector3 = vel + (a1 + 2.0 * a2 + 2.0 * a3 + a4) * (dt / 6.0)
	var out: Array[Vector3] = [new_pos, new_vel]
	return out
```
Сопротивление среды: `accel = Vector3.DOWN * 9.81 - vel * vel.length() * k_drag / mass`.

## Плавучесть (Архимед по точкам), из `_physics_process`
```gdscript
static func apply_buoyancy(body: RigidBody3D, local_points: Array[Vector3], water_y: float, strength: float, damping: float) -> void:
	var n: float = float(local_points.size())
	for local_p in local_points:
		var world_p: Vector3 = body.global_transform * local_p
		var depth: float = water_y - world_p.y
		if depth <= 0.0:
			continue
		var offset: Vector3 = world_p - body.global_position
		var point_vel: Vector3 = body.linear_velocity + body.angular_velocity.cross(offset)
		var force: Vector3 = Vector3.UP * (strength * depth / n) - point_vel * (damping / n)
		body.apply_force(force, offset)
```
Высоту волны считай той же формулой, что в шейдере (функция `gerstner` ниже, в GDScript), иначе лодка «тонет».

## Подвеска на рейкастах (Raycast Vehicle)
```gdscript
static func apply_suspension(body: RigidBody3D, ray: RayCast3D, rest_length: float, stiffness: float, damping: float) -> void:
	if not ray.is_colliding():
		return
	var origin: Vector3 = ray.global_position
	var up: Vector3 = ray.global_transform.basis.y
	var compression: float = rest_length - origin.distance_to(ray.get_collision_point())
	var offset: Vector3 = origin - body.global_position
	var point_vel: Vector3 = body.linear_velocity + body.angular_velocity.cross(offset)
	var force: float = stiffness * compression - damping * point_vel.dot(up)
	body.apply_force(up * force, offset)
```
`ray.target_position = Vector3(0, -rest_length, 0)`; `stiffness * rest_length * 4 ≈ mass * 9.81 * 2`.

## Вода: волна `wave = vec4(dir.x, dir.y, steepness, wavelength)`, сумма крутизн ≤ 1
Нормаль аналитическая: `t` = ∂p/∂x, `b` = ∂p/∂z, `NORMAL = cross(b, t)`. Скелет компилируется (`uniforms: 3`):
```glsl
shader_type spatial;
uniform vec4 wave_a = vec4(1.0, 0.6, 0.25, 12.0);
uniform vec4 wave_b = vec4(-0.4, 1.0, 0.2, 7.0);
uniform vec4 wave_c = vec4(0.8, -0.3, 0.15, 4.0);
varying float crest;

vec3 gerstner(vec4 w, vec3 p, inout vec3 t, inout vec3 b) {
	float k = 2.0 * PI / w.w;
	vec2 d = normalize(w.xy);
	float f = k * (dot(d, p.xz) - sqrt(9.8 / k) * TIME);
	float s = w.z * sin(f);
	float c = w.z * cos(f);
	t += vec3(-d.x * d.x * s, d.x * c, -d.x * d.y * s);
	b += vec3(-d.x * d.y * s, d.y * c, -d.y * d.y * s);
	return vec3(d.x * c, s, d.y * c) / k;
}

void vertex() {
	vec3 t = vec3(1.0, 0.0, 0.0);
	vec3 b = vec3(0.0, 0.0, 1.0);
	vec3 o = gerstner(wave_a, VERTEX, t, b) + gerstner(wave_b, VERTEX, t, b) + gerstner(wave_c, VERTEX, t, b);
	crest = o.y;
	VERTEX += o;
	NORMAL = normalize(cross(b, t));
}

void fragment() {
	float fresnel = pow(1.0 - clamp(dot(NORMAL, VIEW), 0.0, 1.0), 3.0);
	float foam = smoothstep(0.2, 0.4, crest);
	ALBEDO = mix(mix(vec3(0.02, 0.18, 0.4), vec3(0.2, 0.75, 0.85), fresnel), vec3(1.0), foam);
	ROUGHNESS = mix(0.05, 0.6, foam);
}
```
