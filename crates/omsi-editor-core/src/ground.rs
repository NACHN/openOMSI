//! The ground brush: raising, lowering and flattening one tile's height field.
//!
//! A tile's ground is a `cells × cells` grid of samples (`cells + 1` samples each way, one
//! per metre on a stock map) stored in the tile's `.map.terrain` companion. The brush raises
//! the ground under the view within a radius and fades smoothly to nothing at the rim, so
//! repeated presses build a hill instead of a step.
//!
//! Shaping returns exactly which samples it touched and what they were. That is what makes a
//! press undoable without copying a whole tile: a mid-size brush touches a few thousand
//! floats, and the document keeps only those.

use crate::codec::num;
use glam::DVec3;

/// The ground brush's radius when none is set, and its limits (m).
pub const BRUSH_DEFAULT: f64 = 6.0;
pub const BRUSH_MIN: f64 = 1.0;
pub const BRUSH_MAX: f64 = 60.0;

/// What one press of the brush does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GroundAction {
    /// Raise (lower, with a negative) the ground by this many metres.
    Raise(f64),
    /// Bring the ground to this height.
    Flatten(f64),
}

impl GroundAction {
    /// The one-line description a status bar shows.
    pub fn describe(&self) -> String {
        match self {
            GroundAction::Raise(d) if *d >= 0.0 => format!("Ground raised {} m", num(*d)),
            GroundAction::Raise(d) => format!("Ground lowered {} m", num(-*d)),
            GroundAction::Flatten(h) => format!("Ground flattened to {} m", num(*h)),
        }
    }
}

/// A rectangle of height samples inside one tile's grid (indices, not metres).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridRect {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
}

impl GridRect {
    pub fn area(&self) -> usize {
        self.w * self.h
    }
}

/// The samples inside `rect`, row by row - what a change has to put back to be undone.
pub fn capture(t: &omsi_map::Terrain, rect: &GridRect) -> Vec<f32> {
    let n = t.samples();
    let mut out = Vec::with_capacity(rect.area());
    for iy in rect.y0..rect.y0 + rect.h {
        for ix in rect.x0..rect.x0 + rect.w {
            out.push(t.heights.get(iy * n + ix).copied().unwrap_or(0.0));
        }
    }
    out
}

/// Put `values` back into `rect`, as [`capture`] read them. The inverse of a brush press.
pub fn restore(t: &mut omsi_map::Terrain, rect: &GridRect, values: &[f32]) {
    let n = t.samples();
    let mut k = 0;
    for iy in rect.y0..rect.y0 + rect.h {
        for ix in rect.x0..rect.x0 + rect.w {
            if let (Some(v), Some(slot)) = (values.get(k), t.heights.get_mut(iy * n + ix)) {
                *slot = *v;
            }
            k += 1;
        }
    }
}

/// Shape one tile's ground (its origin `o`) about `at` within `r`: raise by [`GroundAction::Raise`]'s
/// metres, or bring toward `Flatten`'s height, weighted smoothly to nothing at the rim.
///
/// Returns which samples moved and what they were *before*, or `None` when nothing moved -
/// a press that finds no ground to change must not land in the undo history.
pub fn shape(
    t: &mut omsi_map::Terrain,
    o: (f64, f64),
    at: DVec3,
    r: f64,
    action: GroundAction,
) -> Option<(GridRect, Vec<f32>)> {
    let n = t.samples();
    if t.cells == 0 || r <= 0.0 {
        return None;
    }
    let step = omsi_map::tile_size() / t.cells as f64;
    if !step.is_finite() || step <= 0.0 {
        return None;
    }
    // The samples that can be within r, so a small brush is a small amount of work.
    let lo = |v: f64, o: f64| ((v - r - o) / step).ceil().max(0.0) as usize;
    let hi = |v: f64, o: f64| {
        let k = ((v + r - o) / step).floor();
        if k < 0.0 {
            None
        } else {
            Some((k as usize).min(n.saturating_sub(1)))
        }
    };
    let x0 = lo(at.x, o.0);
    let y0 = lo(at.y, o.1);
    let (Some(x1), Some(y1)) = (hi(at.x, o.0), hi(at.y, o.1)) else { return None };
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let rect = GridRect { x0, y0, w: x1 - x0 + 1, h: y1 - y0 + 1 };
    let before = capture(t, &rect);

    let mut any = false;
    for iy in y0..=y1 {
        for ix in x0..=x1 {
            let (x, y) = (o.0 + ix as f64 * step, o.1 + iy as f64 * step);
            let d = ((x - at.x).powi(2) + (y - at.y).powi(2)).sqrt();
            if d >= r {
                continue;
            }
            let w = (1.0 - (d / r).powi(2)).powi(2);
            let h = &mut t.heights[iy * n + ix];
            let new = match action {
                GroundAction::Raise(m) => *h as f64 + m * w,
                GroundAction::Flatten(target) => *h as f64 + (target - *h as f64) * w.min(1.0),
            };
            if (new - *h as f64).abs() > 1e-5 {
                *h = new as f32;
                any = true;
            }
        }
    }
    any.then_some((rect, before))
}

/// Where a ray first meets the ground: `aim` from the eye, walking along `forward` in small
/// steps then larger ones, within `max` metres. What the brush uses as "under the cursor".
pub fn aim(ground: impl Fn(f64, f64) -> Option<f64>, eye: DVec3, forward: DVec3, max: f64) -> Option<DVec3> {
    let f = forward.normalize_or_zero();
    if f == DVec3::ZERO {
        return None;
    }
    let mut t = 0.5;
    while t < max {
        let p = eye + f * t;
        if ground(p.x, p.y).is_some_and(|g| p.z <= g) {
            return Some(p);
        }
        t += if t < 50.0 { 0.25 } else { 1.0 };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile-sized, flat, fully untouched ground with `at` in its middle.
    fn flat_at_middle() -> (omsi_map::Terrain, DVec3) {
        let size = omsi_map::tile_size();
        (omsi_map::Terrain::flat(), DVec3::new(size * 0.5, size * 0.5, 0.0))
    }

    #[test]
    fn the_brush_raises_the_middle_most_and_not_its_rim() {
        let (mut t, at) = flat_at_middle();
        let (rect, before) = shape(&mut t, (0.0, 0.0), at, 12.0, GroundAction::Raise(1.0)).unwrap();
        assert!((t.sample(at.x as f32, at.y as f32) - 1.0).abs() < 1e-4);
        assert_eq!(t.sample(at.x as f32 + 18.0, at.y as f32), 0.0);
        assert_eq!(before.len(), rect.area());
        // every sample before the press was untouched ground
        assert!(before.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_flattened_brush_goes_back_where_it_was() {
        let (mut t, at) = flat_at_middle();
        let _ = shape(&mut t, (0.0, 0.0), at, 12.0, GroundAction::Raise(1.0));
        let (rect, before) = shape(&mut t, (0.0, 0.0), at, 12.0, GroundAction::Flatten(0.0)).unwrap();
        assert!(t.sample(at.x as f32, at.y as f32).abs() < 1e-4);
        // putting the samples back restores the raised hill exactly
        restore(&mut t, &rect, &before);
        assert!((t.sample(at.x as f32, at.y as f32) - 1.0).abs() < 1e-4);
        assert_eq!(omsi_map::Terrain::parse(&t.to_bytes()).unwrap(), t);
    }

    #[test]
    fn a_brush_that_moves_nothing_is_not_an_edit() {
        let (mut t, _) = flat_at_middle();
        // far outside the tile: no sample is within reach
        let far = DVec3::new(-500.0, -500.0, 0.0);
        assert!(shape(&mut t, (0.0, 0.0), far, 6.0, GroundAction::Raise(1.0)).is_none());
        // raising by nothing moves nothing either
        let at = DVec3::new(50.0, 50.0, 0.0);
        assert!(shape(&mut t, (0.0, 0.0), at, 6.0, GroundAction::Raise(0.0)).is_none());
    }

    #[test]
    fn capture_and_restore_are_inverses() {
        let mut t = omsi_map::Terrain::flat();
        for (i, h) in t.heights.iter_mut().enumerate() {
            *h = i as f32 * 0.5;
        }
        let rect = GridRect { x0: 3, y0: 4, w: 5, h: 6 };
        let before = capture(&t, &rect);
        assert_eq!(before.len(), 30);
        let n = t.samples();
        for iy in rect.y0..rect.y0 + rect.h {
            for ix in rect.x0..rect.x0 + rect.w {
                t.heights[iy * n + ix] = 999.0;
            }
        }
        restore(&mut t, &rect, &before);
        assert_eq!(capture(&t, &rect), before);
    }
}
