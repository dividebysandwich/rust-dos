// A mesh of the scene: lit by its lights, the sky, the screen's glow and
// the light bounced around (the probes'), fading into the sky with
// distance; or unlit; or the screen, which shows the emulated picture as
// it is.
//
// SHADOW_LAYERS (at least 1), SHADOW_TAPS (1, 6 or 12 samples softening a
// shadow's edge), GRID_W and GRID_H (the screen's patches) are defined
// before this. With BAKE defined, it draws what a probe sees
// instead (see gi.rs's `Seen`).

in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
out vec4 o_color;
#ifdef BAKE
out vec4 o_bake1;
out vec4 o_bake2;
out vec4 o_bake3;
out vec4 o_bake4;
// Whether the surface's back is the inside of something.
uniform int u_backs;
#endif

// 0 lit, 1 unlit, 2 the screen, 3 the test room's floor.
uniform int u_shading;
uniform vec4 u_base_color;
uniform int u_has_base;
uniform sampler2D u_base;
uniform vec3 u_emissive;
uniform int u_has_emissive;
uniform sampler2D u_emissive_map;
// Alpha below which the surface is cut away, or below 0 for none.
uniform float u_cutoff;

uniform vec3 u_ambient;
uniform float u_exposure;
uniform vec3 u_eye[VIEWS];
uniform float u_fog;
uniform int u_lights;
// xyz the position, w the kind: 0 directional, 1 point, 2 spot.
uniform vec4 u_light_pos[8];
uniform vec3 u_light_color[8];
// Where the light shines to; a spot's cone as the cosines of its inner
// and outer angle.
uniform vec3 u_light_dir[8];
uniform vec2 u_light_cone[8];
// The light's first layer of the shadows, or -1 for none; 1 if it has a
// cube's six.
uniform ivec2 u_light_shadow[8];

// How far the nearest surface is, seen from the lights and the screen;
// what each layer sees, and a texel's width on it: x, plus y times the
// distance from the light.
uniform sampler2DArrayShadow u_shadows;
uniform mat4 u_shadow_matrix[SHADOW_LAYERS];
uniform vec2 u_shadow_texel[SHADOW_LAYERS];
uniform float u_shadow_size;

// The picture on the screen, (0, 0) its bottom left.
uniform sampler2D u_screen;
uniform int u_has_screen;
// The screen as a light: the picture's colours in GRID_W by GRID_H
// patches, row 0 the top, in linear light; the glass's top left corner
// and its width and height as vectors going right and down; how bright it
// is. Its shadows are a cube of five faces (the back's left out) from
// `u_glow_point`, turned by `u_glow_basis` (right, up, out of the glass),
// starting at layer `u_glow_shadow` (-1 for none).
uniform sampler2D u_glow_grid;
uniform vec3 u_glow_origin;
uniform vec3 u_glow_across;
uniform vec3 u_glow_down;
uniform float u_glow;
uniform vec3 u_glow_point;
uniform mat3 u_glow_basis;
uniform vec2 u_glow_size;
uniform int u_glow_shadow;

// The light bounced around, at the probes: for each of the axes +x, -x,
// +y, -y, +z, -z a block of the texture as wide as the probes are across,
// its alpha how much the probes there count, the colours times it. The
// first probe's place, how far apart they are, and how many.
uniform int u_has_gi;

// The ambient occlusion over the view (ao.frag), for the light from all
// around: the ambient's and the bounced light. Not for surfaces seen
// through, which aren't in it.
#if VIEWS > 1
uniform sampler2DArray u_ao;
#else
uniform sampler2D u_ao;
#endif
uniform int u_has_ao;
// A see-through mesh, lit more cheaply: no shadows, and the screen's light
// as one light of the whole picture's colour.
uniform int u_see_through;
uniform vec2 u_view_size;
uniform sampler3D u_gi;
uniform vec3 u_gi_low;
uniform vec3 u_gi_step;
uniform vec3 u_gi_count;

// Twelve points spread over the unit disc, for soft shadows.
const vec2 DISC[12] = vec2[](
    vec2(-0.326, -0.406), vec2(-0.840, -0.074), vec2(-0.696, 0.457), vec2(-0.203, 0.621),
    vec2(0.962, -0.195), vec2(0.473, -0.480), vec2(0.519, 0.767), vec2(0.185, -0.893),
    vec2(0.507, 0.064), vec2(0.896, 0.412), vec2(-0.322, -0.933), vec2(-0.792, -0.598));

// How much of a light reaches `p` past what layer `layer` sees, from
// `dist` away, softened over `spread` texels.
float shadow_layer(int layer, vec3 p, vec3 n, float dist, float spread) {
    vec2 t = u_shadow_texel[layer];
    float texel = (t.x + t.y * dist) / u_shadow_size;
    vec4 c = u_shadow_matrix[layer] * vec4(p + n * texel * 1.5, 1.0);
    vec3 s = c.xyz / c.w * 0.5 + 0.5;
    if (c.w <= 0.0 || any(lessThan(s, vec3(0.0))) || any(greaterThan(s, vec3(1.0)))) {
        return 1.0;
    }
    float step = spread / u_shadow_size;
    float sum = 0.0;
    for (int i = 0; i < SHADOW_TAPS; i++) {
        // One sample, or every second of the disc's for six.
        vec2 at = SHADOW_TAPS == 1 ? vec2(0.0) : DISC[i * (12 / SHADOW_TAPS)];
        sum += texture(u_shadows, vec4(s.xy + at * step, float(layer), s.z));
    }
    return sum / float(SHADOW_TAPS);
}

// The face of a cube that direction d goes through: +x, -x, +y, -y, +z,
// -z.
int cube_face(vec3 d) {
    vec3 a = abs(d);
    if (a.x >= a.y && a.x >= a.z) {
        return d.x > 0.0 ? 0 : 1;
    }
    if (a.y >= a.z) {
        return d.y > 0.0 ? 2 : 3;
    }
    return d.z > 0.0 ? 4 : 5;
}

// The cosine-weighted solid angle of the edge from direction a to b (unit
// vectors) as a vector: θ / 2π times the edge's plane's normal (Heitz et
// al.'s fit of θ / (2π sin θ), which needs no acos).
vec3 edge(vec3 a, vec3 b) {
    float x = dot(a, b);
    float y = abs(x);
    float v = (0.8543985 + (0.4965155 + 0.0145206 * y) * y) / (3.4175940 + (4.1616724 + y) * y);
    float theta = x > 0.0 ? v : 0.5 * inversesqrt(max(1.0 - x * x, 1e-7)) - v;
    return cross(a, b) * theta;
}

#define PATCHES (GRID_W * GRID_H)

// How much of each of the screen's patches' light reaches `p`, its surface
// facing `n`: the exact form factor of a Lambertian rectangle, from its
// edges; far from the screen, as a small flat light each, which is all
// but the same.
// Whether `p` is far enough from the screen for its patches to be small
// flat lights.
bool far_from_screen(vec3 p) {
    vec3 middle = u_glow_origin + (u_glow_across + u_glow_down) * 0.5;
    return distance(p, middle) > length(u_glow_across + u_glow_down);
}

// Patch (i, j)'s light at `p` as a small flat light.
float patch_far(vec3 p, vec3 n, int i, int j) {
    vec3 c = u_glow_origin + u_glow_across * ((float(i) + 0.5) / float(GRID_W)) + u_glow_down * ((float(j) + 0.5) / float(GRID_H));
    vec3 to = c - p;
    float d2 = dot(to, to);
    vec3 l = to * inversesqrt(d2);
    float area = length(u_glow_across) * length(u_glow_down) / float(PATCHES) / 3.14159265;
    return area * max(dot(n, l), 0.0) * max(-dot(u_glow_basis[2], l), 0.0) / d2;
}

// Patch (i, j)'s light at `p`, exactly: round its edges, top left to
// right, down, back left, up.
float patch_near(vec3 p, vec3 n, int i, int j) {
    vec3 a = u_glow_origin + u_glow_across * (float(i) / float(GRID_W)) + u_glow_down * (float(j) / float(GRID_H));
    vec3 right = u_glow_across / float(GRID_W);
    vec3 down = u_glow_down / float(GRID_H);
    vec3 v0 = normalize(a - p);
    vec3 v1 = normalize(a + right - p);
    vec3 v2 = normalize(a + right + down - p);
    vec3 v3 = normalize(a + down - p);
    return max(dot(n, edge(v0, v1) + edge(v1, v2) + edge(v2, v3) + edge(v3, v0)), 0.0);
}

void patch_factors(vec3 p, vec3 n, out float f[PATCHES]) {
    if (far_from_screen(p)) {
        for (int j = 0; j < GRID_H; j++) {
            for (int i = 0; i < GRID_W; i++) {
                f[j * GRID_W + i] = patch_far(p, n, i, j);
            }
        }
        return;
    }
    // Patch by patch, from its own corners: what is kept at a time stays
    // small, which keeps the whole shader fast.
    for (int j = 0; j < GRID_H; j++) {
        for (int i = 0; i < GRID_W; i++) {
            f[j * GRID_W + i] = patch_near(p, n, i, j);
        }
    }
}

// The screen's light at `p` as one small flat light of the whole
// picture's colour: all but exact far away, and cheap anywhere (no more
// than a whole hemisphere's worth close by).
vec3 screen_light_one(vec3 p, vec3 n) {
    vec3 to = u_glow_origin + (u_glow_across + u_glow_down) * 0.5 - p;
    float d2 = dot(to, to);
    vec3 l = to * inversesqrt(d2);
    float area = length(u_glow_across) * length(u_glow_down) / 3.14159265;
    float f = area * max(dot(n, l), 0.0) * max(-dot(u_glow_basis[2], l), 0.0) / d2;
    return texelFetch(u_glow_grid, ivec2(GRID_W, 0), 0).rgb * min(f, 1.0);
}

// The light the screen gives `p`: each patch's colour times its factor.
// Far away, as one small flat light of the whole picture's colour.
vec3 screen_light(vec3 p, vec3 n) {
    vec3 middle = u_glow_origin + (u_glow_across + u_glow_down) * 0.5;
    vec3 to = middle - p;
    float d2 = dot(to, to);
    float diagonal = length(u_glow_across + u_glow_down);
    if (d2 > 9.0 * diagonal * diagonal) {
        return screen_light_one(p, n);
    }
    bool far = far_from_screen(p);
    vec3 sum = vec3(0.0);
    for (int j = 0; j < GRID_H; j++) {
        for (int i = 0; i < GRID_W; i++) {
            float f = far ? patch_far(p, n, i, j) : patch_near(p, n, i, j);
            sum += texelFetch(u_glow_grid, ivec2(i, j), 0).rgb * f;
        }
    }
    return sum;
}

// Whether `p` is in front of the glass, where the screen's light goes.
bool before_screen(vec3 p) {
    return dot(p - u_glow_point, u_glow_basis[2]) > -0.02;
}

// How much of the screen's light reaches `p`. Not shadowed close in front
// of the glass, where the bezel's insides are, which the light seen from
// one point would shadow wrongly.
float screen_shadow(vec3 p, vec3 n) {
    if (u_glow_shadow < 0) {
        return 1.0;
    }
    vec3 local = (p - u_glow_point) * u_glow_basis;
    int face = cube_face(local);
    if (face == 5) {
        return 1.0;
    }
    float s = shadow_layer(u_glow_shadow + face, p, n, length(local), 4.0);
    bool inside = all(lessThan(abs(local.xy), u_glow_size * 0.5 + 0.03));
    return inside ? mix(1.0, s, smoothstep(0.02, 0.08, local.z)) : s;
}

// The probes' light along axis `k` at `g` (in probes from the first),
// or nothing where none of the probes around counts.
vec4 gi_axis(vec3 g, int k) {
    vec3 uvw = vec3((float(k) * u_gi_count.x + g.x + 0.5) / (6.0 * u_gi_count.x), (g.yz + 0.5) / u_gi_count.yz);
    vec4 v = texture(u_gi, uvw);
    return v.a > 0.01 ? vec4(v.rgb / v.a, 1.0) : vec4(0.0);
}

// The light bounced around reaching `p`, its surface facing `n`, and how
// much the probes say for it (0 where none counts).
vec4 gi_light(vec3 p, vec3 n) {
    vec3 g = clamp((p + n * 0.1 - u_gi_low) / u_gi_step, vec3(0.0), u_gi_count - 1.0);
    vec3 nn = n * n;
    return nn.x * gi_axis(g, n.x >= 0.0 ? 0 : 1) + nn.y * gi_axis(g, n.y >= 0.0 ? 2 : 3) + nn.z * gi_axis(g, n.z >= 0.0 ? 4 : 5);
}

void main() {
    vec4 base = u_base_color;
#ifndef LEAVE_OUT_TEXTURES
    if (u_has_base == 1) {
        base *= texture(u_base, v_uv);
    }
#endif
    if (u_cutoff >= 0.0 && base.a < u_cutoff) {
        discard;
    }
#ifdef BAKE
    if (u_shading == 2) {
        // The screen's own light is in the lighting already: here it is
        // dark glass.
        o_color = vec4(0.0);
        o_bake1 = o_bake2 = o_bake3 = o_bake4 = vec4(0.0);
        return;
    }
#endif
    if (u_shading == 2) {
        vec3 picture = u_has_screen == 1 ? from_srgb(texture(u_screen, vec2(v_uv.x, 1.0 - v_uv.y)).rgb) : vec3(0.0);
        o_color = finish(picture, 1.0);
        return;
    }
    vec3 emissive = u_emissive;
    if (u_has_emissive == 1) {
        emissive *= texture(u_emissive_map, v_uv).rgb;
    }
    vec3 color;
    if (u_shading == 1) {
        color = base.rgb + emissive;
#ifdef BAKE
        o_color = vec4(color, !gl_FrontFacing && u_backs == 1 ? -1.0 : 0.0);
        o_bake1 = o_bake2 = o_bake3 = o_bake4 = vec4(0.0);
        return;
#endif
    } else {
        vec3 n = normalize(v_normal);
        if (!gl_FrontFacing) {
            n = -n;
        }
        vec3 albedo = base.rgb;
        if (u_shading == 3) {
            // Lines a metre apart, thin at any distance, fading away.
            vec2 g = v_world.xz;
            vec2 a = abs(fract(g - 0.5) - 0.5) / max(fwidth(g), vec2(1e-4));
            float line = (1.0 - min(min(a.x, a.y), 1.0)) * exp(-length(v_world.xz - u_eye[VIEW].xz) * 0.08);
            albedo = mix(albedo, vec3(0.05, 0.07, 0.12), line * 0.8);
            emissive += vec3(0.003, 0.006, 0.014) * line;
        }
        // The sky lights from above more than from below, unless the
        // probes know better.
        vec3 light = u_ambient * (0.65 + 0.35 * n.y);
#ifndef LEAVE_OUT_LIGHTING
#ifndef LEAVE_OUT_GI
        if (u_has_gi == 1) {
            vec4 bounced = gi_light(v_world, n);
            light = mix(light, bounced.rgb, bounced.a);
        }
#endif
#ifndef BAKE
        if (u_has_ao == 1) {
            light *= AO_AT(u_ao, gl_FragCoord.xy / u_view_size).r;
        }
#endif
        for (int i = 0; i < 8; i++) {
            if (i >= u_lights) {
                break;
            }
            vec3 l;
            float att = 1.0;
            float dist = 0.0;
            if (u_light_pos[i].w == 0.0) {
                l = -u_light_dir[i];
            } else {
                vec3 to = u_light_pos[i].xyz - v_world;
                float d2 = max(dot(to, to), 1e-4);
                dist = sqrt(d2);
                l = to / dist;
                att = 1.0 / d2;
                if (u_light_pos[i].w == 2.0) {
                    att *= smoothstep(u_light_cone[i].y, u_light_cone[i].x, dot(-l, u_light_dir[i]));
                }
            }
            float lambert = max(dot(n, l), 0.0) * att;
            ivec2 shadow = u_light_shadow[i];
#ifdef LEAVE_OUT_SHADOWS
            shadow.x = -1;
#endif
            if (u_see_through == 1) {
                shadow.x = -1;
            }
            if (lambert > 0.0 && shadow.x >= 0) {
                int layer = shadow.y == 1 ? shadow.x + cube_face(-l) : shadow.x;
                lambert *= shadow_layer(layer, v_world, n, dist, 1.5);
            }
            light += u_light_color[i] * lambert;
        }
#ifdef BAKE
        float f[PATCHES];
        for (int i = 0; i < PATCHES; i++) {
            f[i] = 0.0;
        }
        if (before_screen(v_world)) {
            patch_factors(v_world, n, f);
            float shadow = screen_shadow(v_world, n);
            for (int i = 0; i < PATCHES; i++) {
                f[i] *= shadow;
            }
        }
        float bright = dot(albedo, vec3(0.2126, 0.7152, 0.0722));
        float total = 0.0;
        vec4 group[3] = vec4[](vec4(0.0), vec4(0.0), vec4(0.0));
        for (int i = 0; i < PATCHES; i++) {
            total += f[i];
            group[i / 4][i % 4] = f[i] * bright;
        }
        o_color = vec4(albedo * light + emissive, !gl_FrontFacing && u_backs == 1 ? -1.0 : 0.0);
        o_bake1 = vec4(albedo * total, bright * total);
        o_bake2 = group[0];
        o_bake3 = group[1];
        o_bake4 = group[2];
        return;
#elif !defined(LEAVE_OUT_SCREEN)
        if (u_has_screen == 1 && u_glow > 0.0 && before_screen(v_world)) {
            if (u_see_through == 1) {
                light += screen_light_one(v_world, n) * u_glow;
            } else {
                vec3 glow = screen_light(v_world, n);
                if (dot(glow, vec3(1.0)) > 0.0) {
#ifdef LEAVE_OUT_SHADOWS
                    light += glow * u_glow;
#else
                    light += glow * u_glow * screen_shadow(v_world, n);
#endif
                }
            }
        }
#endif
#endif
        color = albedo * light + emissive;
    }
    color = tone(color * u_exposure);
    // The haze, where there is one (not in a closed room).
    if (u_fog > 0.0) {
        float dist = length(v_world - u_eye[VIEW]);
        color = mix(color, tone(sky_color(v_world - u_eye[VIEW])), 1.0 - exp(-dist * u_fog));
    }
    o_color = finish(color, base.a);
}
