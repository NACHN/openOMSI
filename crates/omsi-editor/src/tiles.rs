//! The tile grid, drawn over the map while the tile tool is in hand.
//!
//! Where a map begins and ends is not something you can see: the ground is drawn as far as the
//! tiles go, and one more tile of it would look exactly like the ground that is already there.
//! Adding a tile used to mean putting the crosshair on the ground and pressing a button in a
//! panel across the screen from it, or typing the numbers - and taking one away meant finding
//! it in a list. This draws the tiles themselves instead: the ones the map has, ringed in red,
//! and the ring of tiles it could grow into, ringed in white, with the one under the pointer in
//! green. A click on the map is then the whole edit, and what it would do is on the screen
//! before it is made.
//!
//! # Why this is the editor's outline and not geometry
//!
//! The rings are `Scene::outline` - the same marks the object under the pointer and the chosen
//! object are ringed with. The marked instances are drawn into a mask of their own and the ring
//! is the band of picture just outside it, and that is what makes a tile's border the same
//! three pixels wide from any height. A band of flat geometry on the ground could not be: it
//! would be a bright line under the camera and a hair at the far end of the map.
//!
//! It also means none of this needs a material, a colour of its own, or a second drawing path.
//! The colour of a ring *is* the meaning: white for a tile the map could gain, red for one it
//! has, green for the one the pointer is on - the editor's own three, the ones a tool button
//! is drawn in (see [`omsi_render::Mark`]).
//!
//! # The instances are never drawn
//!
//! Each tile gets an ordinary invisible instance of one square mesh, and the mark pass takes it
//! from there: it draws the marked instances itself (with no depth, and no colour but its own -
//! see `shader.wgsl`), so a tile contributes nothing to the picture and its ring is drawn
//! through whatever stands on it. Which is the point: a tile behind a building still has its
//! border, or the grid would have holes in it wherever the map is built up.
//!
//! The instances are a pool, made once and never added to: nothing takes a mesh or an instance
//! back out of a scene, so a pool of a fixed size is what keeps an editor that is left open
//! from growing one. A map may list more tiles than the pool holds - it may list thousands -
//! and the ones nearest the pointer are the ones that are ringed.

use crate::gizmo::Soup;
use glam::{DVec3, Mat4, Vec3};
use omsi_editor_core::{tile_origin, Session, TileId};
use omsi_render::{AlphaMode, Mark, MeshData, Renderer, Scene};
use std::collections::{BTreeSet, HashSet};

/// The gap left between two tiles' marks, as a fraction of a tile.
///
/// Without it the marks of a block of tiles touch, the block is one mark, and one mark has one
/// outline - so the borders *between* the tiles of a block would not be drawn at all, and a
/// block six tiles wide would look like one tile six times too big. A gap of half a percent
/// keeps every tile its own mark; and from high enough up that the gap is thinner than the ring
/// is wide, the rings of a block touch again and read as the grid the tiles are.
const GAP: f64 = 0.005;

/// How many tiles are ringed at once.
///
/// Four hundred and eighty is more than any view of a map needs (the ring around a block is a
/// dozen tiles, and a screenful of a built-up map a few dozen) and small enough that the pool
/// costs nothing: the instances are invisible, so their whole cost is the per-draw entry the
/// renderer uploads for every instance whether it is drawn or not.
const MOST: usize = 480;

/// The tile grid: one mesh, a pool of invisible instances, and the ring of tiles the map could
/// grow into.
#[derive(Default)]
pub struct Overlay {
    built: Option<Built>,
    /// The tiles the map has, and the ring around them - worked out again only when the map's
    /// own set of tiles changes, which is a handful of times in a session and not once a frame.
    tiles: HashSet<TileId>,
    ring: Vec<TileId>,
}

struct Built {
    /// The instances one tile's mark is drawn with. Their mesh is the same square for all of
    /// them, and is reached through them - nothing here needs it by number.
    pool: Vec<usize>,
}

impl Overlay {
    /// Ring the map's tiles, the ring around them, and the one under the pointer.
    ///
    /// `aim` is where the pointer meets the ground this frame - `None` while it is off the map,
    /// which is also when nothing is the tile under the pointer. `from` is where the map is
    /// being looked at from, which decides *which* tiles are ringed when there are more of them
    /// than the pool holds.
    pub fn draw(&mut self, renderer: &Renderer, scene: &mut Scene, session: &Session, aim: Option<DVec3>, from: DVec3) {
        let tiles: HashSet<TileId> = session.doc().tiles().collect();
        if tiles != self.tiles {
            self.ring = ring_of(&tiles);
            self.tiles = tiles;
        }
        let hovered = aim.map(|at| session.tile_at(at));
        let at = aim.unwrap_or(from);

        // the tiles that could be added first, because they are what a click would do, then the
        // ones the map has - each lot from the nearest, and no more than the pool holds
        let mut candidates = self.ring.clone();
        let mut standing: Vec<TileId> = self.tiles.iter().copied().collect();
        // the order decides *which* tiles are ringed only when there are more of them than the
        // pool holds; leaving it alone otherwise keeps the same map the same picture twice
        if candidates.len() + standing.len() > MOST {
            sort_by_distance(&mut candidates, at);
            sort_by_distance(&mut standing, at);
        }

        // The mark is a tile's own square less the gap at each edge, so that what it rings is
        // this tile and not a slice of the block it stands in. The tile under the pointer is
        // the exception, and it is drawn *inside* itself: it is the one tile a click would act
        // on, and a frame at its edge would be one more cell of the grid it stands in. Inside
        // for a tile the map has, and not a colour of its own, because the colour has to go on
        // meaning one thing - green is a click that adds, red a click that takes away, and a
        // tile the map already has is never a click that adds.
        let full = omsi_map::tile_size() * (1.0 - 2.0 * GAP);
        let inside = omsi_map::tile_size() * 0.4;
        let pool = self.ensure(renderer, scene).pool.clone();
        let mut used = 0usize;
        let mut mark_tile = |renderer: &Renderer, scene: &mut Scene, tile: TileId, mark: Mark, side: f64| {
            if used == pool.len() {
                return;
            }
            let scale = Mat4::from_scale(Vec3::new(side as f32, side as f32, 1.0));
            renderer.set_transform(scene, pool[used], centre(tile), scale);
            scene.outline.push((pool[used] as u32, mark));
            used += 1;
        };
        for tile in candidates {
            let mark = if Some(tile) == hovered { Mark::Add } else { Mark::Chosen };
            mark_tile(renderer, scene, tile, mark, full);
        }
        for tile in standing {
            let side = if Some(tile) == hovered { inside } else { full };
            mark_tile(renderer, scene, tile, Mark::Remove, side);
        }
        // The pool is made here - after whatever `prepare` the frame already had - and the
        // tiles are placed after it, so nothing of it is on the GPU yet: an instance's entries
        // are uploaded by `Renderer::prepare` and by nothing else, and an instance made after
        // the last one has no entries at all (its `base` is zero, which points at whatever
        // happens to be first). One more prepare, once a frame, while this tool is in hand, is
        // what puts the grid on the screen at all - and it is also what carries the frame's own
        // moves over, so nothing drawn on the map is a frame behind while the grid is up.
        renderer.prepare(scene);
    }

    /// Forget the marks: they were built against the scene that is being thrown away, and their
    /// mesh and instance numbers mean nothing in the next one.
    pub fn reset(&mut self) {
        self.built = None;
    }

    fn ensure(&mut self, renderer: &Renderer, scene: &mut Scene) -> &Built {
        if self.built.is_none() {
            // the mark pass writes a channel of its own and never reads a material's colour
            // (`shader.wgsl`), but an instance has to have one
            let plain = renderer.add_material(scene, None, AlphaMode::Opaque, [1.0, 1.0, 1.0, 1.0], true);
            let quad = renderer.add_mesh(scene, &square_mesh());
            let pool: Vec<usize> = (0..MOST)
                .map(|_| {
                    let inst = renderer.add_instance(scene, quad, DVec3::ZERO, Mat4::from_scale(Vec3::ZERO), vec![plain]);
                    // a tile is ringed and never drawn, and never casts anything either: the
                    // whole of its life is spent in the outline's mask. It stays *visible* for
                    // that reason - the mask pass collapses an instance that is not (see
                    // `vs_outline`) - and `outline_only` is what keeps it out of the picture.
                    renderer.set_outline_only(scene, inst, true);
                    renderer.set_casts_shadow(scene, inst, false);
                    inst
                })
                .collect();
            self.built = Some(Built { pool });
        }
        self.built.as_ref().expect("just built")
    }
}

/// Where a tile is: the corner the map measures it from, and half of it across.
///
/// `tile_origin` and not arithmetic on the tile's numbers, because that is what the core places
/// a tile's objects by - a tile's square of ground has to be the square its objects stand in.
fn centre(tile: TileId) -> DVec3 {
    let origin = tile_origin(tile);
    let half = omsi_map::tile_size() / 2.0;
    DVec3::new(origin.x + half, origin.y + half, 0.0)
}

/// The tiles next to the ones the map has that it does not have: the ring it could grow into.
fn ring_of(tiles: &HashSet<TileId>) -> Vec<TileId> {
    let mut out: BTreeSet<TileId> = BTreeSet::new();
    for &(x, y) in tiles {
        for dx in -1..=1 {
            for dy in -1..=1 {
                let t = (x + dx, y + dy);
                if !tiles.contains(&t) {
                    out.insert(t);
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Nearest first, so that the pool fills with the tiles being looked at rather than with
/// whichever ones happen to come out of a hash map first.
fn sort_by_distance(tiles: &mut [TileId], at: DVec3) {
    let d = |t: TileId| {
        let c = centre(t);
        (c.x - at.x) * (c.x - at.x) + (c.y - at.y) * (c.y - at.y)
    };
    tiles.sort_by(|a, b| d(*a).total_cmp(&d(*b)));
}

/// A unit square lying flat: what one tile's mark is a scaled copy of.
///
/// Flat, and in the map's own plane, at the height the ground is measured from. Nothing has to
/// be lifted off the ground to be seen: the mask is drawn with no depth at all, so a tile's
/// square cannot lose an argument with the terrain it lies on.
fn square_mesh() -> MeshData {
    let mut soup = Soup::default();
    soup.slot(0);
    let (h, z) = (0.5, 0.0);
    soup.quad(Vec3::new(-h, -h, z), Vec3::new(h, -h, z), Vec3::new(h, h, z), Vec3::new(-h, h, z));
    soup.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(tiles: &[TileId]) -> HashSet<TileId> {
        tiles.iter().copied().collect()
    }

    #[test]
    fn the_ring_is_the_tiles_around_the_map_and_not_the_map_itself() {
        let ring = ring_of(&set(&[(0, 0)]));
        assert_eq!(ring.len(), 8, "{ring:?}");
        assert!(!ring.contains(&(0, 0)), "a tile the map has was offered as one it could gain");
        assert!(ring.contains(&(1, 1)) && ring.contains(&(-1, -1)), "{ring:?}");
    }

    #[test]
    fn two_tiles_side_by_side_offer_six_and_not_ten() {
        // the ring is what is next to *something*, so a tile between two of them is not offered
        // twice - and the ones beside the pair, which are next to neither, are not offered
        let ring = ring_of(&set(&[(0, 0), (1, 0)]));
        assert_eq!(ring.len(), 10, "{ring:?}");
        assert!(!ring.contains(&(0, 0)) && !ring.contains(&(1, 0)));
        assert!(ring.contains(&(-1, 0)) && ring.contains(&(2, 0)));
        assert!(ring.contains(&(0, 1)) && ring.contains(&(1, 1)));
    }

    #[test]
    fn a_block_is_offered_one_ring_and_not_a_ring_round_every_tile() {
        // nine tiles: the eight around the middle are its neighbours and not offered
        let block: Vec<TileId> = (0..3).flat_map(|x| (0..3).map(move |y| (x, y))).collect();
        let ring = ring_of(&set(&block));
        assert!(!ring.iter().any(|t| block.contains(t)), "{ring:?}");
        // the ring round a 3x3 block is the sixteen tiles round it
        assert_eq!(ring.len(), 16, "{ring:?}");
    }

    #[test]
    fn the_nearer_tile_comes_first() {
        let mut tiles = vec![(10, 0), (1, 0), (5, 0)];
        sort_by_distance(&mut tiles, centre((0, 0)));
        assert_eq!(tiles, vec![(1, 0), (5, 0), (10, 0)]);
    }
}
