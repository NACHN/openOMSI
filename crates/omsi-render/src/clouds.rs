//! The noise the enhanced clouds are built from, made once at start-up: the method of
//! Guerrilla's Horizon Zero Dawn clouds as bevy-volumetric-clouds (MIT, evroon) and
//! Frostbite's TileableVolumeNoise build it.
//!
//! * the **shape map** (2-D, tiling): R a Perlin fbm cut by Worley cells - where the heaps
//!   stand and how dense they are; G the coverage the heap needs before it appears there
//!   (three octaves of Worley, stored + 1); B how much its top is rounded off;
//! * the **detail volume** (3-D, tiling): Worley noise of three frequencies that eats the
//!   heaps' edges into billows.
//!
//! Both repeat seamlessly (every lattice is taken modulo its frequency), and both come with
//! a mip chain (a box filter) so that far clouds do not shimmer.

/// Edge of the shape map in texels.
pub const SHAPE_SIZE: u32 = 256;
/// Edge of the detail volume in texels.
pub const DETAIL_SIZE: u32 = 64;

fn fract(x: f32) -> f32 {
    x - x.floor()
}

fn modf(a: f32, b: f32) -> f32 {
    a - b * (a / b).floor()
}

/// Hash without sine (Dave Hoskins), 3 → 1.
fn hash13(p: [f32; 3], k: f32) -> f32 {
    let mut p3 = [fract(p[0] * k), fract(p[1] * k), fract(p[2] * k)];
    let d = p3[0] * (p3[1] + 19.19) + p3[1] * (p3[2] + 19.19) + p3[2] * (p3[0] + 19.19);
    p3 = [p3[0] + d, p3[1] + d, p3[2] + d];
    fract((p3[0] + p3[1]) * p3[2])
}

/// Value noise on a lattice that repeats every `tile` cells.
fn value_noise(x: [f32; 3], tile: f32) -> f32 {
    let p = [x[0].floor(), x[1].floor(), x[2].floor()];
    let f = [fract(x[0]), fract(x[1]), fract(x[2])];
    let u = f.map(|f| f * f * (3.0 - 2.0 * f));
    let h = |dx: f32, dy: f32, dz: f32| {
        hash13([modf(p[0] + dx, tile), modf(p[1] + dy, tile), modf(p[2] + dz, tile)], 0.1031)
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(h(0.0, 0.0, 0.0), h(1.0, 0.0, 0.0), u[0]);
    let x10 = lerp(h(0.0, 1.0, 0.0), h(1.0, 1.0, 0.0), u[0]);
    let x01 = lerp(h(0.0, 0.0, 1.0), h(1.0, 0.0, 1.0), u[0]);
    let x11 = lerp(h(0.0, 1.0, 1.0), h(1.0, 1.0, 1.0), u[0]);
    lerp(lerp(x00, x10, u[1]), lerp(x01, x11, u[1]), u[2])
}

/// Worley (cellular) noise, 1 at a cell's point falling off with the squared distance,
/// on cells that repeat every `tile`.
fn worley(x: [f32; 3], tile: f32) -> f32 {
    let p = [x[0].floor(), x[1].floor(), x[2].floor()];
    let f = [fract(x[0]), fract(x[1]), fract(x[2])];
    let mut best = 100.0f32;
    for k in -1..=1 {
        for j in -1..=1 {
            for i in -1..=1 {
                let b = [i as f32, j as f32, k as f32];
                let c = [modf(p[0] + b[0], tile), modf(p[1] + b[1], tile), modf(p[2] + b[2], tile)];
                // the cell's point, one hash per axis
                let o = [hash13(c, 1031.1031), hash13([c[1] + 7.3, c[2], c[0]], 1031.1031), hash13([c[2] + 3.1, c[0], c[1]], 1031.1031)];
                let r = [b[0] - f[0] + o[0], b[1] - f[1] + o[1], b[2] - f[2] + o[2]];
                best = best.min(r[0] * r[0] + r[1] * r[1] + r[2] * r[2]);
            }
        }
    }
    1.0 - best
}

fn fbm(p: [f32; 3], octaves: u32, freq: f32, noise: fn([f32; 3], f32) -> f32) -> f32 {
    let (mut f, mut amp, mut sum, mut w) = (freq, 1.0, 0.0, 0.0);
    for _ in 0..octaves {
        sum += amp * noise([p[0] * f, p[1] * f, p[2] * f], f);
        w += amp;
        f *= 2.0;
        amp *= 0.5;
    }
    sum / w
}

fn shape_texel(u: f32, v: f32) -> [f32; 3] {
    let c = [u, v, 0.5];
    let perlin = fbm(c, 7, 4.0, value_noise);
    let cells = fbm(c, 4, 8.0, worley);
    let r = (1.0 + (perlin - 1.0) * 0.9) * (1.0 + (cells - 1.0) * 0.7);
    // How much cover a heap needs before it appears (stored + 1): a coarse field, so that it
    // says which stretches of the sky are thick and which are thin instead of cutting one
    // heap into several.
    let g = 0.625 * fbm(c, 3, 15.0, worley) + 0.25 * fbm(c, 3, 19.0, worley) + 0.125 * fbm(c, 3, 23.0, worley) - 1.0;
    let b = 1.0 - fbm([c[0] + 0.5, c[1] + 0.5, c[2] + 0.5], 4, 9.0, worley);
    [r, g + 1.0, b]
}

fn detail_texel(x: f32, y: f32, z: f32) -> f32 {
    let c = [x, y, z];
    let r = fbm(c, 3, 3.0, worley);
    let g = fbm(c, 3, 8.0, worley);
    let b = fbm(c, 3, 16.0, worley);
    (1.0 - (r + g * 0.5 + b * 0.25) / 1.75).max(0.0)
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Rows of `n` computed on as many threads as the machine has.
fn parallel_rows<T: Send + Clone + Default>(n: usize, row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) -> Vec<T> {
    let mut out = vec![T::default(); n * row_len];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).max(1);
    let per = n.div_ceil(threads);
    std::thread::scope(|s| {
        for (k, chunk) in out.chunks_mut(per * row_len).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, row) in chunk.chunks_mut(row_len).enumerate() {
                    f(k * per + i, row);
                }
            });
        }
    });
    out
}

/// The shape map's mip chain, RGBA8, level 0 first.
pub fn shape_map() -> Vec<Vec<u8>> {
    let n = SHAPE_SIZE as usize;
    let base = parallel_rows::<u8>(n, n * 4, |y, row| {
        for x in 0..n {
            let t = shape_texel((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
            row[x * 4..x * 4 + 4].copy_from_slice(&[to_u8(t[0]), to_u8(t[1]), to_u8(t[2]), 255]);
        }
    });
    let mut levels = vec![base];
    let mut size = n;
    while size > 1 {
        let prev = levels.last().expect("level");
        let half = size / 2;
        let mut next = vec![0u8; half * half * 4];
        for y in 0..half {
            for x in 0..half {
                for c in 0..4 {
                    let s: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                        .iter()
                        .map(|(dx, dy)| prev[((2 * y + dy) * size + 2 * x + dx) * 4 + c] as u32)
                        .sum();
                    next[(y * half + x) * 4 + c] = ((s + 2) / 4) as u8;
                }
            }
        }
        levels.push(next);
        size = half;
    }
    levels
}

/// The detail volume's mip chain, R8, level 0 first (z slices of y rows of x).
pub fn detail_volume() -> Vec<Vec<u8>> {
    let n = DETAIL_SIZE as usize;
    let base = parallel_rows::<u8>(n * n, n, |zy, row| {
        let (z, y) = (zy / n, zy % n);
        for (x, v) in row.iter_mut().enumerate() {
            *v = to_u8(detail_texel((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32, (z as f32 + 0.5) / n as f32));
        }
    });
    let mut levels = vec![base];
    let mut size = n;
    while size > 1 {
        let prev = levels.last().expect("level");
        let half = size / 2;
        let mut next = vec![0u8; half * half * half];
        for z in 0..half {
            for y in 0..half {
                for x in 0..half {
                    let mut s = 0u32;
                    for d in 0..8 {
                        let (dx, dy, dz) = (d & 1, (d >> 1) & 1, d >> 2);
                        s += prev[((2 * z + dz) * size + 2 * y + dy) * size + 2 * x + dx] as u32;
                    }
                    next[(z * half + y) * half + x] = ((s + 4) / 8) as u8;
                }
            }
        }
        levels.push(next);
        size = half;
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p omsi-render cloud_report -- --ignored --nocapture`: what the shape
    /// map holds, and how much of the sky each of the tests for "there is a cloud here"
    /// calls a cloud - the one the sky itself uses (`cloud_sigma` over the whole layer,
    /// which is what is drawn) against the single slice a shadow of it on the ground
    /// reads, whose fraction of the ground has to match the sky's.
    #[test]
    #[ignore]
    fn cloud_report() {
        const SIGMA: f32 = 0.035; // sky_enhanced.wgsl CLOUD_SIGMA
        let (bottom, top) = (1400.0f32, 2800.0f32);
        let n = SHAPE_SIZE as usize;
        let mut texels = Vec::new();
        for y in (0..n).step_by(2) {
            for x in (0..n).step_by(2) {
                texels.push(((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32));
            }
        }
        let shapes: Vec<[f32; 3]> = texels.iter().map(|(u, v)| shape_texel(*u, *v)).collect();
        let stat = |name: &str, f: &dyn Fn([f32; 3]) -> f32| {
            let mut v: Vec<f32> = shapes.iter().map(|s| f(*s)).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let q = |p: f32| v[((v.len() - 1) as f32 * p) as usize];
            println!("  {name:<16} min {:>6.3}  p10 {:>6.3}  median {:>6.3}  p90 {:>6.3}  max {:>6.3}", v[0], q(0.1), q(0.5), q(0.9), v[v.len() - 1]);
        };
        println!("shape map, {} texels:", shapes.len());
        stat("r (the heap)", &|s| s[0]);
        stat("lo = g-1", &|s| s[1] - 1.0);
        stat("b (rounding)", &|s| s[2]);

        // (the billows' sideways carry of a heap is left out: it moves the map lookup by a
        // few texels, which shuffles which heap is where without changing the shares below)
        // The heap's own profile at a rise of `alt` metres above the layer's base
        // (sky_enhanced.wgsl): measured in metres, with the layer's base and top only
        // clipping it - not a fraction of the layer, which stretched a heap taller every
        // time a weather raised the top.
        let band = top - bottom;
        let heap = |s: [f32; 3], alt: f32| {
            let h = alt / (1400.0 * (0.6 + 0.7 * s[2]));
            let n = h * h * (0.30 + 0.35 * s[2]) + (1.0 - h.min(1.0)).powi(16);
            let lo = s[1] - 1.0;
            (s[0] - n - lo) / (1.0 - lo).max(1e-3)
        };
        // the layer's bounds: a heap fades in over the band above its base and is cut off
        // over the last of the layer
        let clip = |alt: f32| linear(0.0, 200.0, alt) * (1.0 - linear(band * 0.88, band, alt));
        let erosion = |alt: f32, m: f32, u: f32, v: f32| {
            let dl = detail_texel(u, v, alt / 2000.0);
            m - dl * smoothstep(1.0, 0.5, m) * 0.3
        };
        // what coverage a weather's cover needs for the sky to come out at the share of
        // cloud the type means (the mapping is steep, so it is read off here rather than
        // guessed at)
        if omsi_cfg::env::var_os("CLOUD_SWEEP").is_some() {
            // How open the sky still is at each coverage: "any cloud" (a tenth of the light
            // taken) and "opaque" (nine tenths). The gap between the two is the pale rim; an
            // open sky at a closed cover is what this is read for.
            println!("the sky against `cloud_coverage`'s value:");
            let mut c = 0.44f32;
            while c <= 1.62001 {
                let (mut sky, mut solid) = (0usize, 0usize);
                for (i, s) in shapes.iter().enumerate() {
                    let (u, v) = texels[i];
                    let mut od = 0.0f32;
                    for k in 0..24 {
                        let alt = (k as f32 + 0.5) / 24.0 * band;
                        let mm = heap(*s, alt);
                        if mm + c - 1.0 <= 0.0 {
                            continue;
                        }
                        let me = erosion(alt, mm, u, v);
                        let d = smoothstep(0.0, 0.12, me + c - 1.0) * clip(alt);
                        od += d * SIGMA * band / 24.0;
                    }
                    let t = 1.0 - (-od).exp();
                    if t > 0.1 {
                        sky += 1;
                    }
                    if t > 0.9 {
                        solid += 1;
                    }
                }
                let n = shapes.len() as f32;
                println!("  coverage {c:.3} -> any {:>4.0}%   opaque {:>4.0}%", 100.0 * sky as f32 / n, 100.0 * solid as f32 / n);
                c += 0.06;
            }
            return;
        }
        for cover in [0.35f32, 0.55, 0.75] {
            for field in [-0.125f32, 0.0, 0.125] {
                let cov = (0.408 + cover * 0.2975 + 0.10 * cover.powi(8) + field * 0.16 * (1.0 - cover.powi(4))).clamp(0.0, 1.0);
                let (mut slice, mut eroded, mut sky) = (0usize, 0usize, 0usize);
                for (i, s) in shapes.iter().enumerate() {
                    let (u, v) = texels[i];
                    // (the height a shadow of the layer on the ground reads: 300 m up)
                    let alt = 300.0f32;
                    let m = heap(*s, alt);
                    if m + cov - 1.0 > 0.0 {
                        slice += 1;
                        if erosion(alt, m, u, v) + cov - 1.0 > 0.0 {
                            eroded += 1;
                        }
                    }
                    let mut od = 0.0f32;
                    for k in 0..24 {
                        let alt = (k as f32 + 0.5) / 24.0 * band;
                        let mm = heap(*s, alt);
                        if mm + cov - 1.0 <= 0.0 {
                            continue;
                        }
                        let me = erosion(alt, mm, u, v);
                        let d = smoothstep(0.0, 0.12, me + cov - 1.0) * clip(alt);
                        od += d * SIGMA * band / 24.0;
                    }
                    if 1.0 - (-od).exp() > 0.1 {
                        sky += 1;
                    }
                }
                let t = shapes.len() as f32;
                println!(
                    "cover {cover:.2} field {field:+.3} -> coverage {cov:.3}:  one slice {:>4.0}%   slice+erosion {:>4.0}%   the sky itself {:>4.0}%",
                    100.0 * slice as f32 / t, 100.0 * eroded as f32 / t, 100.0 * sky as f32 / t
                );
            }
        }
    }

    fn linear(a: f32, b: f32, v: f32) -> f32 {
        ((v - a) / (b - a)).clamp(0.0, 1.0)
    }

    fn smoothstep(a: f32, b: f32, v: f32) -> f32 {
        let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    #[test]
    fn noise_tiles_and_spans_its_range() {
        // the lattices repeat: one period on, the same value
        let a = worley([0.3 * 8.0, 0.7 * 8.0, 4.0], 8.0);
        let b = worley([0.3 * 8.0 + 8.0, 0.7 * 8.0, 4.0], 8.0);
        assert!((a - b).abs() < 1e-5);
        let a = value_noise([1.3, 2.7, 0.5], 4.0);
        let b = value_noise([5.3, 2.7, 0.5], 4.0);
        assert!((a - b).abs() < 1e-5);
        let shape = shape_map();
        assert_eq!(shape.len(), SHAPE_SIZE.trailing_zeros() as usize + 1);
        let r: Vec<u8> = shape[0].chunks(4).map(|p| p[0]).collect();
        let (lo, hi) = (r.iter().min().copied().unwrap_or(0), r.iter().max().copied().unwrap_or(0));
        assert!(hi - lo > 120, "shape R spans {lo}..{hi}");
    }
}
