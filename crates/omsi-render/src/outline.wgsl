// The outline the editor draws round a marked object, over the finished picture.
//
// The marked objects were drawn into a mask of their own (`outline_mark` in shader.wgsl), a
// channel to each mark: red under a pointer that would take the object in hand, green for one
// that would put a copy down, blue for one that would take it away, alpha for the object that
// is chosen. The ring is the band of picture just *outside* those pixels, so this reads the
// mask around every pixel and colours the ones that have a marked pixel near them and are not
// marked themselves.
//
// The colour comes of the strongest mark within reach, so what a mark means does not depend on
// the order the objects happened to be drawn in.

struct OutlineParams {
    // xy: one mask texel in uv. zw: one picture pixel in uv.
    texel: vec4<f32>,
    // What each mark rings in, in `Mark`'s order, as the values the picture is drawn with -
    // the editor's own colours (`omsi_render::Mark::colour`), so that the ring and the tool
    // button that put it there cannot disagree.
    colours: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var t_mask: texture_2d<f32>;
@group(0) @binding(1) var s_mask: sampler;
@group(0) @binding(2) var<uniform> params: OutlineParams;

// How wide the ring is, in picture pixels. Three, not one: the outermost samples of the
// search below fall off with distance, so a ring of three leaves a solid two.
const RING: i32 = 3;
// The marks, in `Mark`'s order (see `omsi_render::Mark`).
const TAKE: u32 = 0u;
const ADD: u32 = 1u;
const REMOVE: u32 = 2u;
const CHOSEN: u32 = 3u;

/// Whether a mask pixel holds any mark at all, in any of the four channels.
fn marked(m: vec4<f32>) -> bool {
    return max(max(m.r, m.g), max(m.b, m.a)) > 0.5;
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // one triangle larger than the picture rather than two covering it: no seam down the
    // middle, and one fewer vertex to no purpose
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0));
    var out: VsOut;
    out.pos = vec4<f32>(corners[i], 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // `pos` is in pixels of the picture, and the mask may have been drawn at another size
    // (the render scale), so everything is done in uv: the picture is already the window's
    // width in uv whatever it was drawn at.
    let uv = in.pos.xy * params.texel.zw;
    // `textureSampleLevel` and not `textureSample`: the discard below is control flow the
    // derivatives cannot be taken across, and the mask has one mip anyway.
    let here = textureSampleLevel(t_mask, s_mask, uv, 0.0);
    if (marked(here)) {
        // inside the object: the ring is only what the object does not already cover
        discard;
    }
    var near = 1e9;
    var marks = vec4<f32>(0.0);
    for (var y = -RING; y <= RING; y = y + 1) {
        for (var x = -RING; x <= RING; x = x + 1) {
            if (x == 0 && y == 0) {
                continue;
            }
            let at = uv + vec2<f32>(f32(x), f32(y)) * params.texel.xy;
            let m = textureSampleLevel(t_mask, s_mask, at, 0.0);
            if (marked(m)) {
                let d = length(vec2<f32>(f32(x), f32(y)));
                near = min(near, d);
                marks = max(marks, m);
            }
        }
    }
    if (near > f32(RING)) {
        discard;
    }
    // solid out to a pixel and a half, then falling off - which is what keeps the ring from
    // stepping along a diagonal edge
    let a = 1.0 - smoothstep(f32(RING) - 1.5, f32(RING), near);
    // the most particular thing there is to say about what the ring hugs: a tool that would
    // add or take away says it, then the object being the chosen one, then the neutral mark
    // of a pointer that would take it in hand
    var colour = params.colours[TAKE].rgb;
    if (marks.a > 0.5) {
        colour = params.colours[CHOSEN].rgb;
    }
    if (marks.b > 0.5) {
        colour = params.colours[REMOVE].rgb;
    }
    if (marks.g > 0.5) {
        colour = params.colours[ADD].rgb;
    }
    return vec4<f32>(colour * a, a);
}
