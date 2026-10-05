// What the scene's fragment shaders share: the sunset sky and the way
// linear light becomes the target's values.

// Towards the sun; whether the sky is drawn (else the dark of night);
// whether the target wants sRGB values written (else linear light, which
// an sRGB framebuffer encodes itself).
uniform vec3 u_sun;
uniform int u_sky;
uniform int u_encode;

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

// The sky's light in direction d: dark blue above, an orange band along
// the horizon that is brightest under the sun, and the sun.
vec3 sky_color(vec3 d) {
    d = normalize(d);
    if (u_sky == 0) {
        return vec3(0.003, 0.004, 0.006);
    }
    vec3 sun = normalize(u_sun);
    float h = d.y;
    float toward = dot(normalize(d.xz + vec2(1e-5)), normalize(sun.xz + vec2(1e-5))) * 0.5 + 0.5;
    vec3 zenith = vec3(0.002, 0.005, 0.03);
    vec3 high = vec3(0.012, 0.028, 0.10);
    vec3 orange = vec3(1.0, 0.30, 0.055) * mix(0.18, 1.0, toward * toward * toward);
    vec3 c = mix(high, zenith, smoothstep(0.0, 0.7, h));
    float band = exp(-max(h, 0.0) * mix(16.0, 6.0, toward * toward));
    c = mix(c, orange, band);
    // Below the horizon, haze going dark.
    c = mix(c, vec3(0.004, 0.005, 0.008), smoothstep(0.0, -0.12, h));
    float s = max(dot(d, sun), 0.0);
    c += vec3(1.0, 0.55, 0.2) * (pow(s, 900.0) * 8.0 + pow(s, 30.0) * 0.3);
    return c;
}
