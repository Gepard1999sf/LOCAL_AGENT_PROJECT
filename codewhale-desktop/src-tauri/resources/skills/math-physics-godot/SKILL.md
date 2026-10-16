---
name: math-physics-godot
description: Эталонные физико-математические паттерны для Godot 4.x (GDScript 2.0 и gdshader) — критически затухающая пружина вместо lerp, Верле и RK4, волны Герстнера с нормалями, плавучесть, подвеска на рейкастах, система координат Godot. Use when: пружина, камера следует, отдача, интегрирование, Verlet, RK4, волна, вода, Gerstner, плавучесть, buoyancy, подвеска, raycast vehicle, нормаль, SDF, Y-up, система координат.
invocation: model+user
---

# Математика и физика для Godot 4

Все примеры: статическая типизация, API Godot 4 (не 3), `_physics_process(delta)` для физики.
Точную производную или градиент не выводи «в уме» — вызови `math_eval` с сервера `ai-drive`.

## Система координат
- Godot: **Y вверх, -Z вперёд**, правая система. `Vector3.FORWARD == Vector3(0, 0, -1)`.
- Из Blender (Z вверх, Y вперёд): `godot = Vector3(b.x, b.z, -b.y)`.
- Нормаль из градиента скалярного поля `f(x, y, z)`: `normalize(gradient)`; `math_eval {"mode":"gradient3d", ...}` выдаёт готовый `Vector3(...)`.

## 1. Критически затухающая пружина (камера, отдача, UI) — замена `lerp`
Не зависит от FPS, не перелетает цель (`zeta = 1`). `omega` ≈ `2π / время_отклика_сек`.
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
Использование в `_process(delta)`: `var r := spring_damp(position, target.position, _vel, 12.0, delta); position = r[0]; _vel = r[1]`.

## 2. Интегрирование: Верле и RK4
Явный Эйлер (`pos += vel * dt; vel += acc * dt`) накапливает энергию — не используй для пружин и орбит.
```gdscript
# Verlet: хранит предыдущую позицию, стабилен для верёвок/ткани/цепей.
static func verlet_step(pos: Vector3, prev_pos: Vector3, accel: Vector3, dt: float) -> Array[Vector3]:
	var next: Vector3 = 2.0 * pos - prev_pos + accel * dt * dt
	var out: Array[Vector3] = [next, pos]
	return out

# RK4: accel.call(pos, vel) -> Vector3. Для баллистики с сопротивлением и орбит.
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

## 3. Волны Герстнера с корректной нормалью (`.gdshader`, `shader_type spatial`)
`wave = vec4(dir.x, dir.y, steepness, wavelength)`; сумма крутизн всех волн ≤ 1, иначе петли.
```glsl
vec3 gerstner_wave(vec4 wave, vec3 p, inout vec3 tangent, inout vec3 binormal, float t) {
	float steepness = wave.z;
	float wavelength = wave.w;
	float k = 2.0 * PI / wavelength;
	float c = sqrt(9.8 / k);
	vec2 d = normalize(wave.xy);
	float f = k * (dot(d, p.xz) - c * t);
	float a = steepness / k;
	tangent += vec3(-d.x * d.x * (steepness * sin(f)), d.x * (steepness * cos(f)), -d.x * d.y * (steepness * sin(f)));
	binormal += vec3(-d.x * d.y * (steepness * sin(f)), d.y * (steepness * cos(f)), -d.y * d.y * (steepness * sin(f)));
	return vec3(d.x * (a * cos(f)), a * sin(f), d.y * (a * cos(f)));
}

void vertex() {
	vec3 p = VERTEX;
	vec3 tangent = vec3(1.0, 0.0, 0.0);
	vec3 binormal = vec3(0.0, 0.0, 1.0);
	p += gerstner_wave(wave_a, VERTEX, tangent, binormal, TIME);
	p += gerstner_wave(wave_b, VERTEX, tangent, binormal, TIME);
	VERTEX = p;
	NORMAL = normalize(cross(binormal, tangent));
}
```
Проверяй `godot_check` после каждой правки; `uniforms` в ответе должны совпадать с объявленными.

## 4. Плавучесть (Архимед по точкам), вызывать из `_physics_process`
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
Высоту волны в точке считай той же функцией, что и в шейдере (продублируй формулу в GDScript), иначе лодка «тонет» в видимой воде.

## 5. Подвеска на рейкастах (Raycast Vehicle)
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
`ray.target_position = Vector3(0, -rest_length, 0)`; жёсткость подбирай так, чтобы `stiffness * rest_length * 4 ≈ mass * 9.81 * 2`.

## 6. Частые ошибки локальных моделей
- `move_and_slide()` в Godot 4 без аргументов; скорость в `velocity`.
- `Transform3D.basis.z` — это «назад»; вперёд `-basis.z`.
- В шейдере `int * float` — ошибка: пиши `2.0 * x`, `pow(x, 2.0)`.
- `lerp` каждый кадр — зависимость от FPS; см. пружину выше или `lerp(a, b, 1.0 - exp(-rate * delta))`.
- Для SDF-нормали бери аналитический градиент (`math_eval gradient3d`), а не конечные разности, если формула известна.
