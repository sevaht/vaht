#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// What the source's values mean (crate::color): its transfer function
// (0 sRGB, 1 power, 2 linear, 3 PQ) and the exponent of a power one, and
// the luminance of linear 1, with reference white already scaled to the
// target's.
uniform int source_transfer;
uniform float source_exponent;
uniform float source_scale;
// The source's linear RGB to the target's.
uniform mat3 gamut;
// Tone mapping: whether it runs, where it starts, the brightest light
// coming and the brightest the target shows, in cd/m².
uniform vec4 tone;
// What the target's values mean, as for the source; its scale is the
// linear value of 1 cd/m².
uniform int target_transfer;
uniform float target_exponent;
uniform float target_scale;

// PQ's constants (SMPTE ST 2084).
const float M1 = 2610.0 / 16384.0;
const float M2 = 2523.0 / 4096.0 * 128.0;
const float C1 = 3424.0 / 4096.0;
const float C2 = 2413.0 / 4096.0 * 32.0;
const float C3 = 2392.0 / 4096.0 * 32.0;

vec3 decode(vec3 value, int transfer, float exponent) {
    if (transfer == 0) {
        vec3 magnitude = abs(value);
        vec3 low = magnitude / 12.92;
        vec3 high = pow((magnitude + 0.055) / 1.055, vec3(2.4));
        return sign(value) * mix(high, low, vec3(lessThanEqual(magnitude, vec3(0.04045))));
    } else if (transfer == 1) {
        return pow(max(value, 0.0), vec3(exponent));
    } else if (transfer == 2) {
        return value;
    }
    vec3 power = pow(clamp(value, 0.0, 1.0), vec3(1.0 / M2));
    return pow(max(power - C1, 0.0) / (C2 - C3 * power), vec3(1.0 / M1));
}

vec3 encode(vec3 value, int transfer, float exponent) {
    if (transfer == 0) {
        vec3 magnitude = abs(value);
        vec3 low = magnitude * 12.92;
        vec3 high = 1.055 * pow(magnitude, vec3(1.0 / 2.4)) - 0.055;
        return sign(value) * mix(high, low, vec3(lessThanEqual(magnitude, vec3(0.0031308))));
    } else if (transfer == 1) {
        return pow(max(value, 0.0), vec3(1.0 / exponent));
    } else if (transfer == 2) {
        return value;
    }
    vec3 power = pow(clamp(value, 0.0, 1.0), vec3(M1));
    return pow((C1 + C2 * power) / (1.0 + C3 * power), vec3(M2));
}

// Rolls light above the knee off towards the target's peak, by the
// brightest channel, so hue stays (crate::color::roll_off).
vec3 tone_map(vec3 nits) {
    float brightest = max(nits.r, max(nits.g, nits.b));
    float knee = tone.y;
    if (tone.x < 0.5 || brightest <= knee) {
        return nits;
    }
    float room = tone.w - knee;
    float over = (brightest - knee) / room;
    float most = (tone.z - knee) / room;
    float mapped = knee + room * min(over * (1.0 + over / (most * most)) / (1.0 + over), 1.0);
    return nits * (mapped / brightest);
}

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color.a = 1.0;
#endif
    // Converted unpremultiplied, then premultiplied again.
    vec3 straight = color.a > 0.0 ? color.rgb / color.a : vec3(0.0);
    vec3 nits = decode(straight, source_transfer, source_exponent) * source_scale;
    nits = tone_map(gamut * nits);
    vec3 converted = encode(max(nits * target_scale, 0.0), target_transfer, target_exponent);
    gl_FragColor = vec4(converted * color.a, color.a) * alpha;
}
