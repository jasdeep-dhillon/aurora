#ifdef GL_ES
    // To avoid weird distortion issues when rendering text etc, we want highp if possible.
    // But apparently some devices don't support it, so we have to check first.
    #if defined(GL_FRAGMENT_PRECISION_HIGH) && GL_FRAGMENT_PRECISION_HIGH == 1
        precision highp float;
    #else
        precision mediump float;
    #endif
#endif

uniform sampler2D u_sampler;

// 1 while drawing with the font atlas, whose LCD glyph texels hold per-subpixel
// coverage as (r, g, b, 0) (see epaint `FontData::lcd`).
uniform int u_font_atlas;

#if NEW_SHADER_INTERFACE
    in vec4 v_rgba_in_gamma;
    in vec2 v_tc;
#if DUAL_SOURCE
    // Second output: per-channel blend factor (GL_ONE_MINUS_SRC1_COLOR).
    layout(location = 0, index = 0) out vec4 f_color;
    layout(location = 0, index = 1) out vec4 f_blend;
#else
    out vec4 f_color;
#endif
    // a dirty hack applied to support webGL2
    #define gl_FragColor f_color
    #define texture2D texture
#else
    varying vec4 v_rgba_in_gamma;
    varying vec2 v_tc;
#endif

// -----------------------------------------------
// Adapted from
// https://www.shadertoy.com/view/llVGzG
// Originally presented in:
// Jimenez 2014, "Next Generation Post-Processing in Call of Duty"
//
// A good overview can be found in
// https://blog.demofox.org/2022/01/01/interleaved-gradient-noise-a-different-kind-of-low-discrepancy-sequence/
// via https://github.com/rerun-io/rerun/
float interleaved_gradient_noise(vec2 n) {
    float f = 0.06711056 * n.x + 0.00583715 * n.y;
    return fract(52.9829189 * fract(f));
}

vec3 dither_interleaved(vec3 rgb, float levels) {
    float noise = interleaved_gradient_noise(gl_FragCoord.xy);
    // scale down the noise slightly to ensure flat colors aren't getting dithered
    noise = (noise - 0.5) * 0.95;
    return rgb + noise / (levels - 1.0);
}

void main() {
    vec4 texture_in_gamma = texture2D(u_sampler, v_tc);

    // Premultiplied colors never have color without alpha, so such a texel in
    // the font atlas is LCD subpixel coverage.
    bool lcd = u_font_atlas != 0 && texture_in_gamma.a == 0.0
        && texture_in_gamma.r + texture_in_gamma.g + texture_in_gamma.b > 0.0;

#if DUAL_SOURCE
    // dst = src0 + (1 - src1) * dst, per channel.
    vec4 frag_color_gamma;
    vec4 blend;
    if (lcd) {
        vec3 coverage = texture_in_gamma.rgb;
        float max_coverage = max(coverage.r, max(coverage.g, coverage.b));
        frag_color_gamma = vec4(v_rgba_in_gamma.rgb * coverage, v_rgba_in_gamma.a * max_coverage);
        blend = vec4(v_rgba_in_gamma.a * coverage, frag_color_gamma.a);
    } else {
        // We multiply the colors in gamma space, because that's the only way to get text to look right.
        frag_color_gamma = v_rgba_in_gamma * texture_in_gamma;
        blend = vec4(frag_color_gamma.a);
    }
#else
    if (lcd) {
        // No per-channel blending available: fall back to grayscale coverage.
        texture_in_gamma = vec4(dot(texture_in_gamma.rgb, vec3(1.0 / 3.0)));
    }
    // We multiply the colors in gamma space, because that's the only way to get text to look right.
    vec4 frag_color_gamma = v_rgba_in_gamma * texture_in_gamma;
#endif

    // Dither the float color down to eight bits to reduce banding.
    // This step is optional for egui backends.
#if DITHERING
    frag_color_gamma.rgb = dither_interleaved(frag_color_gamma.rgb, 256.);
#endif
    gl_FragColor = frag_color_gamma;
#if DUAL_SOURCE
    f_blend = blend;
#endif
}
