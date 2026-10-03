// The outline the editor draws round a marked object, over the finished picture.
//
// The marked objects were drawn into a mask of their own (`vs_outline` in shader.wgsl): red
// where the pointer is, green where the object that is chosen is. The ring is the band of
// picture just *outside* those pixels, so this reads the mask around every pixel and colours
// the ones that have a marked pixel near them and are not marked themselves.
//
// The colour follows the strongest mark within reach, which is what makes the chosen object's
// white ring win where the two cross - the hovered object is drawn into the mask first and the
// chosen one over it, so the second reading always stands.

struct OutlineParams {
    // xy: one mask texel in uv. zw: one picture pixel in uv.
    texel: vec4<f32>,
};

@group(0) @binding(0) var t_mask: texture_2d<f32>;
@group(0) @binding(1) var s_mask: sampler;
@group(0) @binding(2) var<uniform> params: OutlineParams;

// How wide the ring is, in picture pixels. Three, not one: the outermost samples of the
// search below fall off with distance, so a ring of three leaves a solid two.
const RING: i32 = 3;
// Under the pointer: OMSI's own amber, the colour of the map's roads on the launcher's
// preview, so that what the editor shows is the same family of colour as what the game does.
const HOVER: vec3<f32> = vec3<f32>(1.0, 0.63, 0.12);
// The object that is chosen.
const CHOSEN: vec3<f32> = vec3<f32>(1.0, 1.0, 1.0);

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
    if (max(here.r, here.g) > 0.5) {
        // inside the object: the ring is only what the object does not already cover
        discard;
    }
    var near = 1e9;
    var hovered = 0.0;
    var chosen = 0.0;
    for (var y = -RING; y <= RING; y = y + 1) {
        for (var x = -RING; x <= RING; x = x + 1) {
            if (x == 0 && y == 0) {
                continue;
            }
            let at = uv + vec2<f32>(f32(x), f32(y)) * params.texel.xy;
            let m = textureSampleLevel(t_mask, s_mask, at, 0.0);
            if (max(m.r, m.g) > 0.5) {
                let d = length(vec2<f32>(f32(x), f32(y)));
                near = min(near, d);
                hovered = max(hovered, m.r);
                chosen = max(chosen, m.g);
            }
        }
    }
    if (near > f32(RING)) {
        discard;
    }
    // solid out to a pixel and a half, then falling off - which is what keeps the ring from
    // stepping along a diagonal edge
    let a = 1.0 - smoothstep(f32(RING) - 1.5, f32(RING), near);
    let colour = select(HOVER, CHOSEN, chosen > 0.5 && chosen >= hovered);
    return vec4<f32>(colour * a, a);
}
