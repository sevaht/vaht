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

// How far apart the samples are, in texture coordinates.
uniform vec2 half_pixel;
// One pixel of the source, in texture coordinates.
uniform vec2 texel;

// A sample at `at`, kept inside the texture: its edges repeat outward
// rather than wrapping to the opposite edge.
vec4 sample(vec2 at) {
    return texture2D(tex, clamp(at, texel * 0.5, vec2(1.0) - texel * 0.5));
}

// Dual Kawase, halving: the pixel four times over and its four diagonal
// neighbours.
void main() {
    vec4 sum = sample(v_coords) * 4.0;
    sum += sample(v_coords - half_pixel);
    sum += sample(v_coords + half_pixel);
    sum += sample(v_coords + vec2(half_pixel.x, -half_pixel.y));
    sum += sample(v_coords - vec2(half_pixel.x, -half_pixel.y));
    vec4 color = sum / 8.0;

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    gl_FragColor = color * alpha;
}
