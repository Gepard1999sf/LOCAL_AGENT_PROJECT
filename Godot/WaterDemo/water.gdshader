// Stylised ocean water for Godot 4 (spatial, GL Compatibility and Forward+).
// Three Gerstner waves displace the mesh and give an analytic normal; the fragment pass
// blends a shallow and a deep colour by scene depth, adds Fresnel, crest and shoreline foam
// and a refracted view of what is under the surface.
shader_type spatial;
render_mode blend_mix, depth_draw_always, cull_back, specular_schlick_ggx;

// wave = (direction.x, direction.y, steepness, wavelength); keep the sum of steepness <= 1.
uniform vec4 wave_a = vec4(1.0, 0.6, 0.25, 12.0);
uniform vec4 wave_b = vec4(-0.4, 1.0, 0.2, 7.0);
uniform vec4 wave_c = vec4(0.8, -0.3, 0.15, 4.0);
uniform float wave_speed : hint_range(0.0, 3.0) = 1.0;
uniform vec3 shallow_color : source_color = vec3(0.2, 0.75, 0.85);
uniform vec3 deep_color : source_color = vec3(0.02, 0.18, 0.4);
uniform float depth_distance : hint_range(0.1, 20.0) = 4.0;
uniform float fresnel_power : hint_range(0.5, 8.0) = 3.0;
uniform float foam_threshold : hint_range(0.0, 1.0) = 0.6;
uniform float refraction : hint_range(0.0, 0.1) = 0.02;
uniform float roughness_value : hint_range(0.0, 1.0) = 0.08;
uniform sampler2D depth_texture : hint_depth_texture, filter_linear_mipmap;
uniform sampler2D screen_texture : hint_screen_texture, filter_linear_mipmap;

varying float crest_factor;

// One Gerstner wave; accumulates the tangent and binormal for the final normal.
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
	float t = TIME * wave_speed;
	p += gerstner_wave(wave_a, VERTEX, tangent, binormal, t);
	p += gerstner_wave(wave_b, VERTEX, tangent, binormal, t);
	p += gerstner_wave(wave_c, VERTEX, tangent, binormal, t);
	// Highest possible crest = sum of amplitudes (steepness / k); 0..1 for the foam mask.
	float max_height = wave_a.z * wave_a.w / (2.0 * PI) + wave_b.z * wave_b.w / (2.0 * PI) + wave_c.z * wave_c.w / (2.0 * PI);
	crest_factor = clamp((p.y - VERTEX.y) / max(max_height, 0.001), 0.0, 1.0);
	VERTEX = p;
	NORMAL = normalize(cross(binormal, tangent));
}

void fragment() {
	// Linear depth of the scene behind the surface versus the surface itself (view space).
	// Forward+/Mobile store reverse-Z in [0,1]; the Compatibility renderer uses OpenGL's
	// [-1,1] NDC, so the sample has to be remapped there or every pixel reads as "shallow".
	float depth_raw = texture(depth_texture, SCREEN_UV).r;
#if CURRENT_RENDERER == RENDERER_COMPATIBILITY
	vec4 depth_view = INV_PROJECTION_MATRIX * vec4(SCREEN_UV * 2.0 - 1.0, depth_raw * 2.0 - 1.0, 1.0);
#else
	vec4 depth_view = INV_PROJECTION_MATRIX * vec4(SCREEN_UV * 2.0 - 1.0, depth_raw, 1.0);
#endif
	depth_view.xyz /= depth_view.w;
	float water_depth = max(-depth_view.z - (-VERTEX.z), 0.0);
	float depth_t = clamp(water_depth / depth_distance, 0.0, 1.0);

	float fresnel = pow(1.0 - clamp(dot(NORMAL, VIEW), 0.0, 1.0), fresnel_power);
	vec3 color = mix(shallow_color, deep_color, depth_t);

	float crest = smoothstep(foam_threshold, 1.0, crest_factor);
	float shore = 1.0 - smoothstep(0.0, 0.35, water_depth);
	float foam = max(crest, shore);
	color = mix(color, vec3(1.0), foam * 0.8);

	vec3 refracted = texture(screen_texture, SCREEN_UV + NORMAL.xz * refraction).rgb;
	ALBEDO = mix(refracted, color, clamp(depth_t * 0.6 + 0.4 + fresnel * 0.3, 0.0, 1.0));
	ALPHA = clamp(0.6 + depth_t * 0.35 + foam, 0.0, 1.0);
	ROUGHNESS = roughness_value;
	SPECULAR = 0.5;
	METALLIC = 0.0;
}
