// The clouds' shadow on the ground, for the enhanced path (see enhanced.wgsl's sun term).
//
// BSL's `GetCloudShadow3` (lib/lighting/shadows.glsl, under its volumetric clouds): a
// surface point's own sun ray is walked up to the cloud layer and what stands in the way
// is what shades the point - so the ground under a heap is dark and the ground in the gap
// beside it stays in the sun, and the pattern moves with the sun and the wind.
//
// What it walks through is `cloud_sigma`, the very function the enhanced sky builds its
// clouds from (sky_enhanced.wgsl), at a handful of heights instead of the sky's fifty-six:
// the same shape map (clouds.rs), the same billows of the detail volume, and the same
// cover - the weather's own cloud picture included. Every one of those has to be in here
// or the shadow stops matching the sky over it.
//
// The ray, and not one slice of it, on purpose. Read at a single height the shadow was a
// threshold on a quantity whose whole range is about a tenth of what the billows then take
// off it, so "cloud" and "no cloud" came out as a step, and every heap cast a shadow with
// the hard edge of a paper cut-out. (The sky escapes this only because it accumulates over
// the whole column and takes whatever the best of its fifty-six steps has to give.) Walking
// the ray instead gives the shadow the sky's own optical depth: soft where a rim is thin
// and opaque through a heap's middle, and thrown as far downwind as the sun's angle says,
// rather than onto the ground directly below.
//
// This module is only compiled when the device can hold three more sampled textures in the
// fragment stage than WebGPU's baseline of 16 (lib.rs, `cloud_shadow`). Where it is not,
// `cloud_shadow_none.wgsl` stands in and the sun simply reaches everywhere.

// The layer stands where the weather puts it (sky_enhanced.wgsl `cloud_base`/`cloud_band`).
fn cloud_base() -> f32 {
    return enh.cloud.x;
}
fn cloud_band() -> f32 {
    return max(enh.cloud.y - enh.cloud.x, 100.0);
}
// The heap's own height above the layer's base, in metres (sky_enhanced.wgsl).
const CLOUD_HEAP_HEIGHT: f32 = 1400.0;
fn cloud_heap_height(b: f32) -> f32 {
    return CLOUD_HEAP_HEIGHT * (0.6 + 0.7 * b);
}

// sky_enhanced.wgsl: the march's own constants, and the three of `cloud_sigma`.
const CLOUD_SIGMA: f32 = 0.035;
const CLOUD_SHAPE_PERIOD: f32 = 13000.0;
const CLOUD_DETAIL_PERIOD: f32 = 750.0;
const CLOUD_DETAIL_STRENGTH: f32 = 0.3;
const CLOUD_BILLOW_CARRY: f32 = 90.0;
const CLOUD_EDGE_SOFTNESS: f32 = 0.12;
const CLOUD_BOTTOM_SOFTNESS: f32 = 200.0;
const CLOUD_TOP_SOFTNESS: f32 = 0.12;
const CLOUD_FIELD_TILE: f32 = 14000.0;
// How many heights of the layer the sun ray is read at. Three would do for a tall layer and
// a little slant; six hold a heap's rim, its middle and the gap beside it apart even when
// the layer is a low overcast and the sun is well down.
const CLOUD_SHADOW_STEPS: i32 = 6;
// How much of the sun a fully opaque column takes away. BSL's own is 0.75 - a cloud's shadow
// is deep, not black, the sky still lights the ground under it - but what it leaves here is
// *direct* sun (the sky's light on the ground is a term of its own, `sh_irradiance`), so a
// quarter of the sun coming through an opaque deck only read as a sunny day with a cloud over
// it: under a cover of 0.8 a quarter of the ground still caught at least a third of the clear
// sky's sun. An opaque column now takes all but a fourteenth.
const CLOUD_SHADOW_STRENGTH: f32 = 0.93;

@group(0) @binding(15) var t_cloud_shape: texture_2d<f32>;
@group(0) @binding(16) var s_cloud: sampler;
@group(0) @binding(20) var t_cloud_detail: texture_3d<f32>;
@group(0) @binding(21) var t_cloud_field: texture_2d<f32>;

// How much of the sky the weather covers here (sky_enhanced.wgsl `cloud_coverage`): its type
// (camera.clouds.x) and, over tens of kilometres, its own cloud picture. This is the height
// a heap has to reach to be drawn at all, and the ground has to read it too or a clear
// region would be shaded while the sky above it shows blue.
fn cloud_coverage(p: vec2<f32>) -> f32 {
    let cover = camera.clouds.x;
    let uv = p / CLOUD_FIELD_TILE + camera.clouds.yz * (2500.0 / CLOUD_FIELD_TILE);
    let field = textureSampleLevel(t_cloud_field, s_cloud, uv * 0.35, 5.0).g;
    // (the same curve the sky reads, so the ground is shaded by the cover that is drawn: see
    // sky_enhanced.wgsl `cloud_coverage`, where the two constants are measured)
    let field_w = 0.16 * (1.0 - pow(cover, 4.0));
    return clamp(0.408 + cover * 0.2975 + 0.10 * pow(cover, 8.0) + (field - 0.5) * field_w, 0.0, 1.0);
}

// Extinction (1/m) where the ray is, exactly as the sky reads it (sky_enhanced.wgsl
// `cloud_sigma`, with the billows always on).
fn cloud_sigma(p: vec3<f32>, coverage: f32, footprint: f32) -> f32 {
    let h = (p.z - cloud_base()) / cloud_band();
    if (h <= 0.0 || h >= 1.0) {
        return 0.0;
    }
    // The billows come first: besides eating the heaps' edges they carry each heap sideways
    // as it rises, so this has to be fetched before the shape map is read.
    let q = vec3<f32>(p.xy + camera.clouds.yz * 2500.0 * 1.3, p.z) / CLOUD_DETAIL_PERIOD;
    // (the same two mips the sky reads at, from the footprint of a ground pixel up there:
    // the shadow's own pattern is a slow one, and reading the billows sharp only aliases it)
    let shape_lod = log2(max(footprint * f32(textureDimensions(t_cloud_shape).x) / CLOUD_SHAPE_PERIOD, 1.0));
    let detail_lod = log2(max(footprint * f32(textureDimensions(t_cloud_detail).x) / CLOUD_DETAIL_PERIOD, 1.0));
    let billow = textureSampleLevel(t_cloud_detail, s_cloud, q, detail_lod).r;
    let carry = CLOUD_BILLOW_CARRY * (billow - 0.5);
    let drift = camera.clouds.yz * 2500.0;
    let s = textureSampleLevel(t_cloud_shape, s_cloud, (p.xy + drift + vec2<f32>(carry, carry * 0.35)) / CLOUD_SHAPE_PERIOD, shape_lod);
    let lo = s.g - 1.0;
    let rise = (p.z - cloud_base()) / cloud_heap_height(s.b);
    let n = rise * rise * (0.30 + 0.35 * s.b) + pow(1.0 - clamp(rise, 0.0, 1.0), 16.0);
    var m = (s.r - n - lo) / max(1.0 - lo, 1e-3);
    if (m + coverage - 1.0 <= 0.0) {
        return 0.0;
    }
    m = m - billow * smoothstep(1.0, 0.5, m) * CLOUD_DETAIL_STRENGTH;
    m = smoothstep(0.0, CLOUD_EDGE_SOFTNESS, m + coverage - 1.0);
    // the layer's bounds: a heap fades in over the band above its base and is cut off at
    // the top
    let cut = 1.0 - clamp((h - (1.0 - CLOUD_TOP_SOFTNESS)) / CLOUD_TOP_SOFTNESS, 0.0, 1.0);
    m = m * min((p.z - cloud_base()) / CLOUD_BOTTOM_SOFTNESS, 1.0) * cut;
    return m * CLOUD_SIGMA;
}

// 1 = the sun reaches this point unshaded, less where a heap stands between it and the sun.
fn cloud_shadow(world: vec3<f32>) -> f32 {
    let cover = camera.clouds.x;
    let sd = normalize(camera.sun_dir.xyz);
    // (a sun at the horizon throws this out to the horizon for very little, and the layer's
    // own soft edge is what shades a town at dusk anyway)
    if (cover <= 0.001 || sd.z <= 0.02) {
        return 1.0;
    }
    let base = cloud_base();
    let band = cloud_band();
    // The stretch of the sun ray that lies inside the layer, from this point up. Its
    // horizontal reach is the layer's height times the cotangent of the sun's elevation, so
    // a low sun lays the shadow out to the side and a noon sun nearly under the heap.
    let origin = vec3<f32>(world.xy + camera.world_origin.zw, world.z);
    let s0 = max((base - world.z) / sd.z, 0.0);
    let s1 = (base + band - world.z) / sd.z;
    if (s1 <= s0) {
        return 1.0;
    }
    let ds = (s1 - s0) / f32(CLOUD_SHADOW_STEPS);
    // the weather's own picture is one value over the whole ray (a field of tens of km)
    let coverage = cloud_coverage((origin + sd * (s0 + s1) * 0.5).xy);
    var od = 0.0;
    for (var i = 0; i < CLOUD_SHADOW_STEPS; i = i + 1) {
        let p = origin + sd * (s0 + (f32(i) + 0.5) * ds);
        od = od + cloud_sigma(p, coverage, 0.0) * ds;
    }
    return 1.0 - (1.0 - exp(-od)) * CLOUD_SHADOW_STRENGTH;
}
