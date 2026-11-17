#version 100
// Background blur ("vibrancy"). Samples a pre-rendered, 1/4-size copy of
// the wallpaper with a 25-tap Gaussian; the downsampling already removes
// most detail, so one pass looks smooth. Clipped like `rounded.frag`.

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

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

uniform vec4 geo;      // window rect: x, y, w, h (framebuffer px)
uniform float radius;  // corner radius (framebuffer px)
uniform vec2 texel;    // 1 / blur texture size
uniform float tint_strength; // white veil on top, like macOS materials

void main() {
    vec2 half_size = geo.zw * 0.5;
    vec2 q = abs(gl_FragCoord.xy - (geo.xy + half_size)) - (half_size - vec2(radius));
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    float mask = clamp(0.5 - d, 0.0, 1.0);

    vec4 color = vec4(0.0);
    float total = 0.0;
    for (int x = -2; x <= 2; x++) {
        for (int y = -2; y <= 2; y++) {
            vec2 o = vec2(float(x), float(y));
            float w = exp(-dot(o, o) / 4.5);
            color += texture2D(tex, v_coords + o * texel * 1.5) * w;
            total += w;
        }
    }
    color /= total;
    color.rgb = mix(color.rgb, vec3(1.0), tint_strength);
    color = vec4(color.rgb, 1.0) * alpha * mask;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
