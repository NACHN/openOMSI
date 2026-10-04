// Enhanced graphics: the sky - the physical sky table, clouds lit by the sun and the sky,
// the sun's disc and the air - and the reflection probe drawn from it.

// Clouds: a ray-marched volume on the curved Earth, built the way Guerrilla's Horizon Zero
// Dawn clouds are (and bevy-volumetric-clouds and Frostbite after them):
// * the layer is a spherical shell 1400-2800 m over the ground of an Earth of 6371 km, so
//   the clouds sink to the horizon and thin out into the distance as real ones do;
// * the shape map (clouds.rs: Perlin cut by Worley cells, repeating every 13 km) says where
//   the heaps stand and how much cover each needs to appear; its blue channel and a height
//   gradient round their tops and flatten their bases; the weather's cover (Cumulus 1-3,
//   Overcast) and the weather's own cloud picture nudge how much of it appears;
// * the detail volume (3-D Worley billows, repeating every 420 m) eats the edges, most at
//   the thin rims and least in the dense cores;
// * light: the sun through the cloud towards it (six steps growing 1.7 times, 1.4 km in
//   all) with Hillaire's approximation of multiple scattering (three octaves of weaker
//   extinction and a flatter phase), a two-lobe Henyey-Greenstein phase (a silver lining
//   towards the sun, a little back-scatter away from it), the sky's light from above and
//   the ground's from below by height, integrated per step the energy-conserving
//   Frostbite way; far clouds take on the air's colour.
// The sky cube keeps the result in a fixed world frame (lib.rs `SKY_CUBE_SIZE`): each
// redraw starts the steps at another random point and is blended into what is there, so
// the grain of a few dozen steps averages out over the frames.
// The layer stands where the weather puts it (`Lighting::cloud_base/cloud_top`, from
// `weather_setup::cloud_layer_of`): 1400-2800 m by default, a storm's three-kilometre
// pile-up or an overcast's low ceiling otherwise.
fn cloud_base() -> f32 {
    return enh.cloud.x;
}
fn cloud_band() -> f32 {
    return max(enh.cloud.y - enh.cloud.x, 100.0);
}
const EARTH_R: f32 = 6371000.0;
// How many steps a ray takes through the layer. This is also the finest the volume can be
// read at all: with 56 over a three-kilometre layer a step is fifty metres, and the billow
// volume's finest octave (64 cycles to its period - twelve metres at a period of 750) went
// through it four times too fast. What that leaves is a regular comb down the steps, and each
// redraw's new jitter walks it about, which is the shimmer on a still sky - seen only once a
// player zooms in far enough for one cube texel to be worth several pixels. A step has to be
// about as short as the finest lobe a detail period is asked for: 750 m means a 12 m octave,
// which is a hundred and ninety-two steps over a three-kilometre layer.
const CLOUD_STEPS: i32 = 192;
// Extinction per metre of the densest cloud.
const CLOUD_SIGMA: f32 = 0.035;
const CLOUD_SHAPE_PERIOD: f32 = 13000.0;
// The billows that eat the heaps' edges. Their period sets the size of the lobes a heap
// comes out in: at 420 m the volume's three octaves carved it into 140, 52 and 26 m balls,
// and a cloud two kilometres across looked like a heap of peas rather than one cloud.
// (Read at its own scale in z as well: the volume is 64 slices over the same 2000 m, and
// multiplying z by three to keep the billows changing up a tall layer put its 16-cycle
// octave at 42 m, which combed every cloud with visible layer lines.)
const CLOUD_DETAIL_PERIOD: f32 = 750.0;
const CLOUD_DETAIL_STRENGTH: f32 = 0.3;
// How far the billows carry a heap sideways between the base of the layer and its top (m).
const CLOUD_BILLOW_CARRY: f32 = 90.0;
const CLOUD_EDGE_SOFTNESS: f32 = 0.12;
// The heap's own height above the layer's base, in metres, and the band at the layer's
// base and top it fades in over and is cut off over. The profile is measured in metres and
// not as a fraction of the layer on purpose: `cloud_base`/`cloud_top` say where cloud may
// stand at all, they do not stretch what stands there. Taken as a fraction, raising the top
// drew the same heap taller - a cloud two kilometres across and three high, pulling the
// billows with it into vertical streaks.
const CLOUD_HEAP_HEIGHT: f32 = 1400.0;
// Every heap takes a share of that from the map's rounding channel, so a layer tall enough
// holds small puffs and towering cumulus together (about 800 m to 1.8 km).
fn cloud_heap_height(b: f32) -> f32 {
    return CLOUD_HEAP_HEIGHT * (0.6 + 0.7 * b);
}
const CLOUD_BOTTOM_SOFTNESS: f32 = 200.0;
const CLOUD_TOP_SOFTNESS: f32 = 0.12;
const CLOUD_MAX_DIST: f32 = 60000.0;
const CLOUD_MS_GAIN: f32 = 2.4;

@group(1) @binding(6) var t_cloud_shape: texture_2d<f32>;
@group(1) @binding(7) var t_cloud_detail: texture_3d<f32>;
@group(1) @binding(8) var s_cloud: sampler;

// Where in its first step this pixel's ray starts (0..1; 0.5 for the reflection probe).
var<private> cloud_jitter: f32 = 0.5;

fn linearstep(a: f32, b: f32, v: f32) -> f32 {
    return clamp((v - a) / (b - a), 0.0, 1.0);
}

// Height over the curved ground of the point t metres along d from the camera.
fn cloud_height(d: vec3<f32>, t: f32) -> f32 {
    return camera.cam_pos.z + eye_off.z + d.z * t + t * t * (1.0 - d.z * d.z) / (2.0 * EARTH_R);
}

// Distance along d at which the ray reaches height h over the curved ground (the far root
// when the camera is above h; -1 when it never does).
fn cloud_shell(d: vec3<f32>, h: f32) -> f32 {
    let a = (1.0 - d.z * d.z) / (2.0 * EARTH_R);
    let b = d.z;
    let c = camera.cam_pos.z + eye_off.z - h;
    if (a < 1e-12) {
        return select(-1.0, -c / b, abs(b) > 1e-6 && -c / b > 0.0);
    }
    let disc = b * b - 4.0 * a * c;
    if (disc < 0.0) {
        return -1.0;
    }
    let q = -0.5 * (b + select(-1.0, 1.0, b >= 0.0) * sqrt(disc));
    let r0 = q / a;
    let r1 = c / q;
    let lo = min(r0, r1);
    let hi = max(r0, r1);
    if (lo > 0.0) {
        return lo;
    }
    return select(-1.0, hi, hi > 0.0);
}

// How much of the sky the weather covers here: the weather's own cover (camera.clouds.x) and,
// over tens of kilometres, its own cloud picture. This is the height a heap has to reach to be
// drawn at all, so it is the one number that says how much blue is left, and it is read almost
// straight. Measured against the sky itself - a wide view upward, counting the pixels that are
// still clearly blue - the coverage a cover needs runs about 0.41 + 0.30 x cover, so a tenth of
// the slider is a tenth of the sky and half of it half the sky. (The earlier curves were read
// off `cloud_report`, which turned out to be a poor model of the sky: 0.545 + 0.155 sqrt put a
// cover of 0.1 at two fifths of the sky - "16 % is a clear sky and 18 % is a whole deck", as a
// player put it - with the middle of the slider moving nothing at all.)
//
// The top of the range is carried further, to shut the sky: heaps leave gaps between them
// whatever the cover says, and without the 0.10 x cover^8 term a cover of 1.0 still left a
// tenth of the sky open - a hole with the sun to see by, and a bright patch of ground under it
// with the edge of a shadow around it, while the weather called the sky closed. With it the
// coverage reaches 0.81 there, which `cloud_report` reads as every texel of the map carrying
// cloud. The weather's own cloud picture - the field below, which swings the cover either way
// over tens of kilometres - is faded out as the cover closes, having no gaps left to swing it
// into.
fn cloud_coverage(p: vec2<f32>) -> f32 {
    let cover = camera.clouds.x;
    let uv = p / CLOUD_FIELD_TILE + camera.clouds.yz * (2500.0 / CLOUD_FIELD_TILE);
    let field = textureSampleLevel(t_clouds, s_repeat, uv * 0.35, 5.0).g;
    let field_w = 0.16 * (1.0 - pow(cover, 4.0));
    return clamp(0.408 + cover * 0.2975 + 0.10 * pow(cover, 8.0) + (field - 0.5) * field_w, 0.0, 1.0);
}

// The heap before its billows, and the billow that carves it - returned together, so the
// caller need not fetch it again. The heap is as tall as it is, wherever the layer's top
// happens to be (see CLOUD_HEAP_HEIGHT).
fn cloud_base_shape(p: vec3<f32>, footprint: f32) -> vec2<f32> {
    let drift = camera.clouds.yz * 2500.0;
    // Each map is read at the mip its own texels ask for: a pixel covers `footprint` metres
    // of cloud, and the shape map is 256 texels over 13 km (51 m each) against the billow
    // volume's 64 over the detail period (12 m at 750). Taking the volume at the shape's own
    // level - `lod - 2`, as it was - left it four mips too sharp, and the billows it read
    // there were aliased: a regular diagonal comb over every distant cloud, and the march's
    // per-redraw jitter walked it about, which is the shimmer on a still sky. (The carry
    // below turns that aliasing into a wobble of the heap's edge, at 90 m of it.)
    let shape_lod = log2(max(footprint * f32(textureDimensions(t_cloud_shape).x) / CLOUD_SHAPE_PERIOD, 1.0));
    let detail_lod = log2(max(footprint * f32(textureDimensions(t_cloud_detail).x) / CLOUD_DETAIL_PERIOD, 1.0));
    // The billows come first: besides eating the heaps' edges they carry each heap
    // sideways as it rises. Held back, every heap was the same column from its base to its
    // top, and the layer read as one flat sheet however tall it was drawn.
    let q = vec3<f32>(p.xy + camera.clouds.yz * 2500.0 * 1.3, p.z) / CLOUD_DETAIL_PERIOD;
    let billow = textureSampleLevel(t_cloud_detail, s_cloud, q, detail_lod).r;
    let carry = CLOUD_BILLOW_CARRY * (billow - 0.5);
    let s = textureSampleLevel(t_cloud_shape, s_cloud, (p.xy + drift + vec2<f32>(carry, carry * 0.35)) / CLOUD_SHAPE_PERIOD, shape_lod);
    let lo = s.g - 1.0;
    // The heap narrows upwards, and the map rounds its top off (a heap where it was small
    // rose as a column with a flat lid). It narrows far more gently than it used to: with
    // the old profile a heap was spent by the middle of the layer, and 400 m of cloud over
    // a heap two kilometres across is a pancake.
    let rise = (p.z - cloud_base()) / cloud_heap_height(s.b);
    let n = rise * rise * (0.30 + 0.35 * s.b) + pow(1.0 - clamp(rise, 0.0, 1.0), 16.0);
    let m = (s.r - n - lo) / (1.0 - lo);
    return vec2<f32>(m, billow);
}

// Extinction (1/m) at p; `detail` false for the light towards the sun far from the point.
fn cloud_sigma(p: vec3<f32>, h: f32, coverage: f32, footprint: f32, detail: bool) -> f32 {
    if (h <= 0.0 || h >= 1.0) {
        return 0.0;
    }
    let shape = cloud_base_shape(p, footprint);
    var m = shape.x;
    // (the billows only take away)
    if (m + coverage - 1.0 <= 0.0) {
        return 0.0;
    }
    if (detail) {
        m = m - shape.y * smoothstep(1.0, 0.5, m) * CLOUD_DETAIL_STRENGTH;
    }
    m = smoothstep(0.0, CLOUD_EDGE_SOFTNESS, m + coverage - 1.0);
    // the layer's bounds: a heap fades in over the band above its base and is cut off at
    // the top - a heap taller than the layer loses its lid there, as one would
    let rise = p.z - cloud_base();
    let cut = 1.0 - linearstep(1.0 - CLOUD_TOP_SOFTNESS, 1.0, h);
    m = m * min(rise / CLOUD_BOTTOM_SOFTNESS, 1.0) * cut;
    return m * CLOUD_SIGMA;
}

// The clouds towards d in front of `below` (the sky behind them): rgb the picture, a how
// much the clouds cover.
fn cloud_layer(d: vec3<f32>, below: vec3<f32>, pix: f32) -> vec4<f32> {
    if (camera.clouds.x <= 0.001 || d.z <= -0.01 || camera.cam_pos.z + eye_off.z > cloud_base()) {
        return vec4<f32>(below, 0.0);
    }
    let sd = normalize(camera.sun_dir.xyz);
    // How closed the cover is: it hides the sun's disc (the alpha this returns) and holds
    // back the high thin layer. It no longer paints a flat grey card over the clouds - the
    // weather that a closed cover comes with now stands as a low, thick, unlit layer of
    // real cloud (`cloud_layer_of`), which is the dark ceiling a rainy sky has.
    let closed = max(smoothstep(0.85, 1.0, camera.clouds.x), enh.weather.w);
    let t0 = cloud_shell(d, cloud_base());
    if (t0 < 0.0 || t0 > CLOUD_MAX_DIST) {
        return vec4<f32>(below, 0.0);
    }
    let t1 = min(cloud_shell(d, cloud_base() + cloud_band()), min(t0 + 24000.0, CLOUD_MAX_DIST + 6000.0));
    let ds = (t1 - t0) / f32(CLOUD_STEPS);
    // How many metres of cloud one pixel covers where the ray meets the layer; both maps are
    // read at their own mip from this (see `cloud_base_shape`). The march's own step is *not*
    // taken into it: a step is the resolution along the ray, and folding it in here blurred
    // the map sideways as well, at a step of fifteen metres to a screen pixel of four - the
    // clouds went flat. What keeps the along-ray side honest is the step count against the
    // volume's finest octave (`CLOUD_STEPS`).
    let footprint = t0 * pix;
    // (the sun before the clouds: how much of it the cover lets through, lights.w, is what
    // this march works out itself)
    let sun = enh.sun_disc.rgb * smoothstep(-0.08, 0.02, sd.z);
    // The light a heap takes from above: read through the cover over it, because a closed sky
    // has already spent most of it before it reaches the layer's underside - which is what
    // makes an overcast's base the grey of a storm cloud, while fair-weather cumulus keeps its
    // bright tops. (Squared: it is a closed cover that shows, not a scattered one.)
    let over = camera.clouds.x;
    let sky_top = sh_irradiance(vec3<f32>(0.0, 0.0, 1.0)) / PI * (1.0 - 0.65 * over * over);
    let ground = sh_irradiance(vec3<f32>(0.0, 0.0, -1.0)) / PI;
    let cos_sun = dot(d, sd);
    let coverage = cloud_coverage(cloud_ground(d, t0));
    var trans = 1.0;
    var acc = vec3<f32>(0.0);
    var hit = 0.0;
    var hit_w = 0.0;
    var t = t0 + ds * cloud_jitter;
    for (var i = 0; i < CLOUD_STEPS; i = i + 1) {
        let p = vec3<f32>(cloud_ground(d, t), cloud_height(d, t));
        let h = (p.z - cloud_base()) / cloud_band();
        let sigma = cloud_sigma(p, h, coverage, footprint, true);
        if (sigma > 1e-6) {
            // The sunlight reaching p through the cloud towards the sun. The steps have to
            // carry the ray across the layer - six of them growing 1.7 times reach 17 x the
            // first, so the first is a fourteenth of the layer's height. Fixed at 40 m they
            // covered 774 m of the old 1400 m layer and only a quarter of a storm's: the
            // depth sat at full within the first few steps every time, the whole inside of a
            // heap came out as lit as its rim, and a cloud had no side to be seen.
            var od = 0.0;
            var ls = CLOUD_HEAP_HEIGHT / 14.0;
            var lt = ls * 0.5;
            for (var k = 0; k < 6; k = k + 1) {
                let q = p + sd * lt;
                let hq = (q.z - cloud_base()) / cloud_band();
                if (hq >= 1.0) {
                    break;
                }
                od = od + cloud_sigma(q, hq, coverage, footprint * 2.0, k < 2) * ls;
                ls = ls * 1.7;
                lt = lt + ls;
            }
            // multiple scattering (Hillaire 2016): octaves of weaker extinction, weaker
            // light and a flatter phase
            var direct = vec3<f32>(0.0);
            var a = 1.0;
            var b = 1.0;
            var c = 1.0;
            for (var o = 0; o < 3; o = o + 1) {
                let phase = mix(hg_phase(cos_sun, 0.8 * c), hg_phase(cos_sun, -0.2 * c), 0.5);
                direct = direct + sun * a * phase * exp(-od * b);
                a = a * 0.5;
                b = b * 0.4;
                c = c * 0.5;
            }
            // (single scattering and three octaves hold only part of the light a cloud
            // scatters on inside it; a cumulus's sunlit side is about as bright as white
            // paper in the sun, E/π, which this factor brings it to)
            let amb = mix(ground * 0.45 + sky_top * 0.55, sky_top * 1.1, clamp(h * 1.4, 0.0, 1.0));
            let light = direct * CLOUD_MS_GAIN + amb;
            // Frostbite: the light scattered over the step, dimmed by the cloud before it
            let dt = exp(-sigma * ds);
            acc = acc + trans * light * (1.0 - dt);
            hit = hit + t * trans * (1.0 - dt);
            hit_w = hit_w + trans * (1.0 - dt);
            trans = trans * dt;
            if (trans < 0.01) {
                break;
            }
        }
        t = t + ds;
    }
    // far clouds take on the colour of the air in front of them, and beyond 40 km fade out
    let dist = select(t0, hit / max(hit_w, 1e-4), hit_w > 1e-4);
    let aerial = 1.0 - exp(-dist / 22000.0);
    let fade = 1.0 - smoothstep(40000.0, CLOUD_MAX_DIST, dist);
    let horizon_fade = smoothstep(-0.01, 0.02, d.z);
    let a = (1.0 - trans) * fade * horizon_fade;
    acc = mix(acc, below * (1.0 - trans), aerial) * fade * horizon_fade;
    var col = acc + (1.0 - a) * below;
    // the high, thin layer
    let drift = camera.clouds.yz * 2500.0;
    let t_hi = (cloud_base() + 6.0 * cloud_band()) / max(d.z, 0.02);
    let p_hi = cloud_ground(d, t_hi);
    let hi = cloud_fbm((p_hi + drift * 1.7) / 2500.0);
    let hi_cover = clamp((hi - 0.6 + camera.clouds.x * 0.2) * 2.0, 0.0, 1.0) * clamp(d.z * 8.0, 0.0, 1.0) * 0.35 * (1.0 - a) * (1.0 - closed);
    col = mix(col, (enh.sun_disc.rgb * enh.lights.w * max(sd.z, 0.0) * 0.6 / PI + sky_top) * 0.9, hi_cover);
    return vec4<f32>(col, max(max(a, hi_cover), closed));
}

// Sky radiance towards d without the sun's disc: the table, the clouds, the air.
fn sky_radiance(d: vec3<f32>, pix: f32) -> vec3<f32> {
    var col = sky_table(d);
    let c = cloud_layer(d, col, pix);
    col = c.rgb;
    // the weather's fog along the whole way to the horizon (the clear air is in the table
    // already: taken again, it whitened the whole sky)
    let h0 = camera.cam_pos.z - enh.fog.z;
    let dist = 30000.0;
    let a = air_of(d, dist, h0, h0 + dist * max(d.z, 0.0), 0.0);
    return col * a.a + a.rgb;
}

// The sky as the sky cube holds it (drawn a face a frame by `fs_sky_cube`, divided by the
// table scale): rgb, and how much cloud covers the direction.
@group(0) @binding(17) var t_sky_cube: texture_cube<f32>;

@fragment
fn fs_enhanced(in: VsOut) -> @location(0) vec4<f32> {
    let d = normalize(in.dir);
    let pre = enh.exposure.x;
    // the cube is drawn from its own eye (lib.rs Probe::cube_eye): look the clouds' base up
    // from there, so the sky does not slide with a camera that moved since
    var ld = d;
    // (The cube is drawn from an eye that the camera leaves behind between two captures, and
    // this re-projects onto it. Taken against the layer's *base* that is exact for a heap's
    // foot and off by `offset x (1/base - 1/cloud)` for anything above it - at three
    // kilometres, and 140 m of driving, four degrees for the top of a tall layer: the clouds
    // slid along with the camera and snapped back at every capture. The lower-middle is where
    // most of a heap's mass stands, so it spreads that error over the layer instead.)
    let tb = cloud_shell(d, cloud_base() + cloud_band() * 0.4);
    if (tb > 0.0 && camera.cam_pos.z < cloud_base()) {
        ld = normalize(d * min(tb, CLOUD_MAX_DIST) - enh.eye.xyz);
    }
    let cube = textureSampleLevel(t_sky_cube, s_lin, vec3<f32>(ld.x, ld.z, ld.y), 0.0);
    var col = cube.rgb * enh.ground.w;
    // the sun: a limb-darkened disc as bright as its irradiance spread over its size,
    // behind whatever cloud there is
    let sd = normalize(camera.sun_dir.xyz);
    let r = enh.sun.w;
    let cosang = dot(d, sd);
    if (cosang > cos(r * 1.5) && d.z > -0.02) {
        let x = clamp(acos(clamp(cosang, -1.0, 1.0)) / r, 0.0, 1.5);
        let disc = 1.0 - smoothstep(0.85, 1.1, x);
        let limb = 1.0 - 0.6 * (1.0 - sqrt(max(1.0 - min(x, 1.0) * min(x, 1.0), 0.0)));
        let cover = cube.a;
        let h0 = camera.cam_pos.z - enh.fog.z;
        let t = air_of(d, 30000.0, h0, h0 + 30000.0 * max(d.z, 0.0), 0.0).a;
        let l = enh.sun_disc.rgb / (PI * r * r) * disc * limb * (1.0 - cover) * enh.lights.w * t;
        col = col + l;
    }
    // the dome is drawn pre-exposed; the disc is kept within what the target and the glow
    // filter handle
    return vec4<f32>(min(col * pre, vec3<f32>(4000.0)), 1.0);
}

// --- the reflection probe: a cube map of the sky seen from the camera, drawn now and then
// and blurred into its mip levels for rough surfaces (divided by the table scale, which
// the main pass multiplies back in).
struct ProbeParams {
    // x first face of this pass (0 or 3), y roughness, z face size of level 0, w level
    p: vec4<f32>,
};
@group(0) @binding(15) var<uniform> probe: ProbeParams;
@group(0) @binding(16) var t_probe_src: texture_cube<f32>;

// Direction of a texel of cube face f (u, v in -1..1, v down), in cube coordinates.
fn face_dir(f: i32, u: f32, v: f32) -> vec3<f32> {
    var c = vec3<f32>(-u, -v, -1.0);
    switch f {
        case 0: { c = vec3<f32>(1.0, -v, -u); }
        case 1: { c = vec3<f32>(-1.0, -v, u); }
        case 2: { c = vec3<f32>(u, 1.0, v); }
        case 3: { c = vec3<f32>(u, -1.0, -v); }
        case 4: { c = vec3<f32>(u, -v, 1.0); }
        default: {}
    }
    return normalize(c);
}

fn cube_to_world(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x, c.z, c.y);
}

struct ProbeOut {
    @location(0) a: vec4<f32>,
    @location(1) b: vec4<f32>,
    @location(2) c: vec4<f32>,
};

struct FsIn {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_probe(@builtin(vertex_index) i: u32) -> FsIn {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return FsIn(vec4<f32>(x, y, 0.0, 1.0));
}

fn probe_sky(f: i32, uv: vec2<f32>) -> vec4<f32> {
    let c = face_dir(f, uv.x, uv.y);
    let d = cube_to_world(c);
    var col: vec3<f32>;
    if (d.z >= 0.0) {
        // the sky cube (lib.rs binds it as level 0's source), already divided by the table
        // scale: marching the clouds again for the probe took 3.4 ms every time it was drawn
        return vec4<f32>(textureSampleLevel(t_probe_src, s_lin, c, 0.0).rgb, 1.0);
    } else {
        // below the horizon: the table's distant ground, with the houses and trees of a
        // town darkening the band just under the horizon
        col = sky_table(d);
        let band = smoothstep(0.02, 0.25, -d.z);
        col = mix(col, enh.ground.rgb * 0.8, 1.0 - band * 0.6);
    }
    return vec4<f32>(col / max(enh.ground.w, 1e-8), 1.0);
}

@fragment
fn fs_probe_sky(in: FsIn) -> ProbeOut {
    let size = probe.p.z;
    let uv = in.clip.xy / size * 2.0 - vec2<f32>(1.0);
    let f0 = i32(probe.p.x);
    return ProbeOut(probe_sky(f0, uv), probe_sky(f0 + 1, uv), probe_sky(f0 + 2, uv));
}

fn radical_inverse(i: u32) -> f32 {
    var b = i;
    b = (b << 16u) | (b >> 16u);
    b = ((b & 0x55555555u) << 1u) | ((b & 0xAAAAAAAAu) >> 1u);
    b = ((b & 0x33333333u) << 2u) | ((b & 0xCCCCCCCCu) >> 2u);
    b = ((b & 0x0F0F0F0Fu) << 4u) | ((b & 0xF0F0F0F0u) >> 4u);
    b = ((b & 0x00FF00FFu) << 8u) | ((b & 0xFF00FF00u) >> 8u);
    return f32(b) * 2.3283064365386963e-10;
}

// GGX-filtered radiance around n (the split-sum assumption: view = normal), sampling the
// sharper levels at a lod that matches each sample's footprint.
fn prefilter(n: vec3<f32>) -> vec3<f32> {
    let rough = probe.p.y;
    let a = rough * rough;
    let up = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let tx = normalize(cross(up, n));
    let ty = cross(n, tx);
    let count = 32u;
    let texel_sa = 4.0 * PI / (6.0 * probe.p.z * probe.p.z);
    var sum = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let xi = vec2<f32>(f32(i) / f32(count), radical_inverse(i));
        let phi = 2.0 * PI * xi.x;
        let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
        let sin_t = sqrt(1.0 - cos_t * cos_t);
        let h = tx * (cos(phi) * sin_t) + ty * (sin(phi) * sin_t) + n * cos_t;
        let l = 2.0 * dot(n, h) * h - n;
        let nl = dot(n, l);
        if (nl <= 0.0) {
            continue;
        }
        let nh = max(dot(n, h), 0.0);
        let d2 = nh * nh * (a * a - 1.0) + 1.0;
        let dd = a * a / (PI * d2 * d2);
        let pdf = dd * 0.25;
        let sample_sa = 1.0 / (f32(count) * pdf + 1e-4);
        let lod = clamp(0.5 * log2(sample_sa / texel_sa) + 1.0, 0.0, probe.p.w - 1.0);
        sum = sum + textureSampleLevel(t_probe_src, s_lin, l, lod).rgb * nl;
        wsum = wsum + nl;
    }
    return sum / max(wsum, 1e-4);
}

@fragment
fn fs_probe_filter(in: FsIn) -> ProbeOut {
    let size = probe.p.z / pow(2.0, probe.p.w);
    let uv = in.clip.xy / size * 2.0 - vec2<f32>(1.0);
    let f0 = i32(probe.p.x);
    return ProbeOut(
        vec4<f32>(prefilter(face_dir(f0, uv.x, uv.y)), 1.0),
        vec4<f32>(prefilter(face_dir(f0 + 1, uv.x, uv.y)), 1.0),
        vec4<f32>(prefilter(face_dir(f0 + 2, uv.x, uv.y)), 1.0),
    );
}

// One face of the sky cube: the sky radiance (the table, the clouds, the air) divided by the
// table scale, and the cloud cover in alpha (the sun's disc is hidden by it).
@fragment
fn fs_sky_cube(in: FsIn) -> @location(0) vec4<f32> {
    let size = probe.p.z;
    let uv = in.clip.xy / size * 2.0 - vec2<f32>(1.0);
    let d = cube_to_world(face_dir(i32(probe.p.x), uv.x, uv.y));
    let pix = 1.4 / size;
    // a new start point for the steps each redraw (p.y counts the redraws): interleaved
    // gradient noise over the face, shifted by the golden ratio
    let ign = fract(52.9829189 * fract(dot(in.clip.xy, vec2<f32>(0.06711056, 0.00583715))));
    cloud_jitter = fract(ign + probe.p.y * 0.61803399);
    eye_off = enh.eye.xyz;
    var col = sky_table(d);
    let c = cloud_layer(d, col, pix);
    col = c.rgb;
    let h0 = camera.cam_pos.z + eye_off.z - enh.fog.z;
    let dist = 30000.0;
    let a = air_of(d, dist, h0, h0 + dist * max(d.z, 0.0), 0.0);
    col = col * a.a + a.rgb;
    return vec4<f32>(col / max(enh.ground.w, 1e-8), c.a);
}
