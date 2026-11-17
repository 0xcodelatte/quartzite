#version 100
// Texture shader that clips client buffers to a rounded rectangle.
// Used as an override for every surface of a window, so subsurfaces are
// clipped too. `geo` is in framebuffer pixels (gl_FragCoord space).

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

void main() {
    vec2 half_size = geo.zw * 0.5;
    vec2 q = abs(gl_FragCoord.xy - (geo.xy + half_size)) - (half_size - vec2(radius));
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    float mask = clamp(0.5 - d, 0.0, 1.0);

    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    color = color * alpha * mask;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
