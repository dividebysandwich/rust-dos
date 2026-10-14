// What the scene's fragment shaders share: the sky at the time of day and
// the way linear light becomes the target's values.

// Towards the sun and the moon; whether the sky is drawn (else the dark of
// night); whether the target wants sRGB values written (else linear
// light, which an sRGB framebuffer encodes itself).
uniform vec3 u_sun;
uniform vec3 u_moon;
uniform int u_sky;
uniform int u_encode;
// The sky's colours (sky.rs's `SkyLook`): high up, along the horizon, the
// glow under the sun at its rising and setting, the sun's and the moon's
// discs; how many stars show, how overcast it is, and a lightning flash.
uniform vec3 u_zenith;
uniform vec3 u_horizon;
uniform vec3 u_band;
uniform vec3 u_sun_disc;
uniform vec3 u_moon_disc;
uniform float u_stars;
uniform float u_clouds;
uniform float u_flash;

vec3 from_srgb(vec3 c) {
    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

vec3 to_srgb(vec3 c) {
    c = clamp(c, 0.0, 1.0);
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c));
}

// Light as it is up to 0.9, then rolling off to 1: bright lights and the
// sun don't clip, and the picture's colours stay all but exact.
vec3 tone(vec3 c) {
    vec3 over = max(c - 0.9, 0.0);
    return min(c, 0.9) + 0.1 * (1.0 - exp(-over / 0.1));
}

vec4 finish(vec3 linear, float alpha) {
    return vec4(u_encode == 1 ? to_srgb(linear) : linear, alpha);
}

// A number from 0 to 1 for a cell of space.
float hash3(vec3 p) {
    p = fract(p * vec3(0.1031, 0.1030, 0.0973));
    p += dot(p, p.yxz + 33.33);
    return fract((p.x + p.y) * p.z);
}

// The sky's light in direction d without the sun, the moon and the stars:
// the colour high up going into the horizon's, the glow along the horizon
// brightest under the sun. What the haze fades into.
vec3 sky_haze(vec3 d) {
    d = normalize(d);
    if (u_sky == 0) {
        return vec3(0.003, 0.004, 0.006);
    }
    vec3 sun = normalize(u_sun);
    float h = d.y;
    float toward = dot(normalize(d.xz + vec2(1e-5)), normalize(sun.xz + vec2(1e-5))) * 0.5 + 0.5;
    vec3 c = mix(u_horizon, u_zenith, smoothstep(0.0, 0.7, h));
    vec3 band = u_band * mix(0.18, 1.0, toward * toward * toward);
    c = mix(c, band, exp(-max(h, 0.0) * mix(16.0, 6.0, toward * toward)) * min(dot(u_band, vec3(1.0)) * 4.0, 1.0));
    c = mix(c, u_horizon * 0.25, smoothstep(0.0, -0.12, h));
    return c + vec3(0.55, 0.6, 0.8) * u_flash * (0.6 + 0.4 * h);
}

// The sky's light in direction d: the haze's, the stars, the sun and the
// moon, greyed by clouds.
vec3 sky_color(vec3 d) {
    d = normalize(d);
    if (u_sky == 0) {
        return vec3(0.003, 0.004, 0.006);
    }
    vec3 sun = normalize(u_sun);
    float h = d.y;
    float toward = dot(normalize(d.xz + vec2(1e-5)), normalize(sun.xz + vec2(1e-5))) * 0.5 + 0.5;
    vec3 c = mix(u_horizon, u_zenith, smoothstep(0.0, 0.7, h));
    vec3 band = u_band * mix(0.18, 1.0, toward * toward * toward);
    c = mix(c, band, exp(-max(h, 0.0) * mix(16.0, 6.0, toward * toward)) * min(dot(u_band, vec3(1.0)) * 4.0, 1.0));
    // The stars: a few cells of the sky lit, twinkling a little.
    if (u_stars > 0.0 && h > 0.0) {
        vec3 cell = floor(d * 450.0);
        float star = step(0.9965, hash3(cell));
        vec3 middle = (cell + 0.5) / 450.0;
        float near = smoothstep(0.0011, 0.0, length(d - normalize(middle)));
        float bright = hash3(cell + 7.0);
        c += vec3(0.75, 0.8, 1.0) * star * near * u_stars * smoothstep(0.0, 0.15, h) * (0.15 + 0.85 * bright * bright);
    }
    // Below the horizon, haze going dark.
    c = mix(c, u_horizon * 0.25, smoothstep(0.0, -0.12, h));
    // The sun: big and soft low in the haze, small and sharp up high.
    float s = max(dot(d, sun), 0.0);
    float sharp = mix(900.0, 25000.0, smoothstep(0.12, 0.45, sun.y));
    c += u_sun_disc * (pow(s, sharp) * 8.0 + pow(s, 30.0) * 0.3 * (1.0 - u_clouds));
    float m = max(dot(d, normalize(u_moon)), 0.0);
    c += u_moon_disc * (smoothstep(0.99985, 0.99992, m) * 1.2 + pow(m, 200.0) * 0.02);
    c += vec3(0.55, 0.6, 0.8) * u_flash * (0.6 + 0.4 * h);
    return c;
}
