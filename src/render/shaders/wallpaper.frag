// Procedural wallpaper: a soft macOS-style gradient with a few glowing
// blobs. Drawn as the bottom-most element; it is static, so damage
// tracking only repaints it where windows move.
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float blob(vec2 p, vec2 c, float r) {
    float d = length((p - c) * vec2(size.x / size.y, 1.0));
    return exp(-d * d / (r * r));
}

void main() {
    vec2 p = v_coords;
    vec3 top = vec3(0.12, 0.16, 0.38);
    vec3 bottom = vec3(0.36, 0.17, 0.42);
    vec3 col = mix(top, bottom, p.y);
    col += vec3(0.95, 0.45, 0.30) * 0.55 * blob(p, vec2(0.78, 0.72), 0.35);
    col += vec3(0.25, 0.55, 0.95) * 0.50 * blob(p, vec2(0.18, 0.30), 0.30);
    col += vec3(0.85, 0.35, 0.75) * 0.35 * blob(p, vec2(0.50, 0.95), 0.25);
    gl_FragColor = vec4(col, 1.0) * alpha;
}
