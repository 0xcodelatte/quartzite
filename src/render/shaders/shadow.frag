// Soft drop shadow for a rounded rectangle (Smithay pixel shader).
// The element covers the window plus a margin; we evaluate a blurred
// rounded-rect SDF analytically, so no offscreen blur pass is needed.
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

varying vec2 v_coords;   // 0..1 across the element
uniform vec2 size;       // element size in physical px (provided by Smithay)
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 area;       // element size, logical px
uniform vec4 rect;       // window rect inside the element: x, y, w, h (logical)
uniform float radius;    // window corner radius (logical)
uniform float sigma;     // blur radius (logical)
uniform float offset_y;  // shadows fall slightly downward, like macOS
uniform float strength;  // opacity of the darkest part

float rrect_sdf(vec2 p, vec4 r, float rad) {
    vec2 half_size = r.zw * 0.5;
    vec2 q = abs(p - (r.xy + half_size)) - (half_size - vec2(rad));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - rad;
}

// Abramowitz–Stegun style erf approximation, good to ~1e-3.
float erf_approx(float x) {
    float s = sign(x);
    float a = abs(x);
    float t = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    t *= t;
    return s - s / (t * t);
}

void main() {
    vec2 p = v_coords * area;
    float d = rrect_sdf(p - vec2(0.0, offset_y), rect, radius);
    float shadow = 0.5 - 0.5 * erf_approx(d / (sigma * 1.41421356));
    // Don't paint under the window itself (it may be translucent).
    float inside = clamp(0.5 - rrect_sdf(p, rect, radius), 0.0, 1.0);
    float a = shadow * strength * (1.0 - inside) * alpha;
    gl_FragColor = vec4(0.0, 0.0, 0.0, a);
}
