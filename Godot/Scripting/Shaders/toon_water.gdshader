shader_type spatial;
render_mode blend_mix, cull_back, depth_draw_always, specular_schlick_ggx;

uniform vec4 shallow_color : source_color = vec4(0.22, 0.78, 0.86, 1.0);
uniform vec4 deep_color : source_color = vec4(0.03, 0.22, 0.45, 1.0);
uniform vec4 foam_color : source_color = vec4(1.0, 1.0, 1.0, 1.0);
uniform float wave_speed : hint_range(0.0, 5.0) = 1.0;
uniform float wave_height : hint_range(0.0, 2.0) = 0.15;
uniform float wave_scale = 0.6;
uniform float depth_fade : hint_range(0.1, 10.0) = 2.5;
uniform float foam_width : hint_range(0.01, 2.0) = 0.35;
uniform float fresnel_power : hint_range(0.5, 10.0) = 4.0;
uniform int toon_steps : hint_range(1, 8) = 3;
uniform sampler2D depth_texture : hint_depth_texture, filter_linear_mipmap;

float wave(vec2 p, float t) {
	return sin(p.x * wave_scale + t) * 0.6 + sin(p.y * wave_scale * 1.3 + t * 1.7) * 0.4;
}

void vertex() {
	float t = TIME * wave_speed;
	vec3 world = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
	VERTEX.y += wave(world.xz, t) * wave_height;
	float e = 0.1;
	float hx = wave(world.xz + vec2(e, 0.0), t) - wave(world.xz - vec2(e, 0.0), t);
	float hz = wave(world.xz + vec2(0.0, e), t) - wave(world.xz - vec2(0.0, e), t);
	NORMAL = normalize(vec3(-hx * wave_height / (2.0 * e), 1.0, -hz * wave_height / (2.0 * e)));
}

void fragment() {
	float raw = texture(depth_texture, SCREEN_UV).r;
	vec4 view = INV_PROJECTION_MATRIX * vec4(SCREEN_UV * 2.0 - 1.0, raw, 1.0);
	view.xyz /= view.w;
	float diff = max(-view.z - (-VERTEX.z), 0.0);
	float depth_t = clamp(diff / depth_fade, 0.0, 1.0);
	float fresnel = pow(1.0 - clamp(dot(NORMAL, VIEW), 0.0, 1.0), fresnel_power);
	float shade = clamp(depth_t + fresnel * 0.5, 0.0, 1.0);
	shade = floor(shade * float(toon_steps)) / float(toon_steps);
	vec3 color = mix(shallow_color.rgb, deep_color.rgb, shade);
	float foam = 1.0 - step(foam_width, diff);
	color = mix(color, foam_color.rgb, foam);
	ALBEDO = color;
	ALPHA = mix(0.75, 1.0, max(depth_t, foam));
	ROUGHNESS = 0.15;
	SPECULAR = 0.4;
}
