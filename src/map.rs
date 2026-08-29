// src/map.rs
//! Map module.
//!
//! Responsibilities:
//! - Generate an open, boundless world from deterministic value noise.
//! - Persist player-caused tile mutations (harvesting, building) as an
//!   overlay on top of the procedural terrain.
//! - Provide isometric projection helpers.
//! - Load and cache map textures.
//! - Stream terrain chunks in and out based on what the camera can see, so
//!   tiles outside the viewport are never spawned.
//! - Maintain terrain depth ordering.
//! - Convert mouse coordinates back to grid coordinates.
//! - Highlight the currently hovered tile.
//! - React to tile mutation events.
//! - Provide circle-vs-tile collision queries ("hitboxes") used by movement.
//! - Keep the camera following the player through the open world.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::events::TileChangedEvent;
use crate::pawn::PlayerPawn;

pub const TILE_WIDTH: f32 = 64.0;
pub const TILE_HEIGHT: f32 = 32.0;

pub const Z_LAYER_TERRAIN: f32 = 0.0;
pub const Z_LAYER_STRUCTURE: f32 = 0.01;
pub const Z_LAYER_PAWN: f32 = 0.02;
pub const Z_LAYER_HIGHLIGHT: f32 = 100.0;

/// Tiles are streamed in square chunks of this size (in tiles).
pub const CHUNK_SIZE: i32 = 16;

/// Extra chunks kept loaded beyond the visible range before they're
/// despawned again. Without this, a chunk sitting exactly on the visible
/// boundary would load and unload every frame as the camera drifts by a
/// fraction of a tile.
const CHUNK_UNLOAD_MARGIN_CHUNKS: i32 = 1;

/// Extra tiles (beyond the exact visible rectangle) that count as visible,
/// so terrain finishes streaming in slightly before it reaches the screen
/// edge instead of visibly popping in.
const VISIBLE_TILE_MARGIN: f32 = 4.0;

/// How quickly the camera catches up to the player. Larger = snappier.
const CAMERA_FOLLOW_RATE: f32 = 8.0;

/// A single logical map tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Grass,
    Tree,
    Rock,
    Wall,
    Bed,
}

impl Tile {
    pub fn is_walkable(self) -> bool {
        matches!(self, Tile::Grass | Tile::Bed)
    }

    pub fn is_harvestable(self) -> bool {
        matches!(self, Tile::Tree | Tile::Rock)
    }

    pub fn texture_path(self) -> &'static str {
        match self {
            Tile::Grass => "textures/grass.png",
            Tile::Tree => "textures/tree.png",
            Tile::Rock => "textures/rock.png",
            Tile::Wall => "textures/wall.png",
            Tile::Bed => "textures/bed.png",
        }
    }
}

/// The open, boundless world.
///
/// Terrain is never stored in bulk. Instead, any tile's type is derived on
/// demand from deterministic noise seeded once at world creation. Player
/// actions (harvesting, building) are recorded as overrides in a sparse map,
/// so a chunk that unloads and later reloads still remembers what happened
/// to it.
#[derive(Resource, Clone)]
pub struct MapData {
    pub seed: u64,
    pub overrides: HashMap<(i32, i32), Tile>,
}

impl Default for MapData {
    fn default() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};

        // A fresh random seed each run, so every world is a new open world.
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0x5EED_1234_ABCD_EF00);

        Self {
            seed,
            overrides: HashMap::new(),
        }
    }
}

impl MapData {
    /// The world has no edges.
    pub fn in_bounds(&self, _x: i32, _y: i32) -> bool {
        true
    }

    pub fn get(&self, x: i32, y: i32) -> Tile {
        self.overrides
            .get(&(x, y))
            .copied()
            .unwrap_or_else(|| generate_tile(self.seed, x, y))
    }

    pub fn set(&mut self, x: i32, y: i32, tile: Tile) {
        self.overrides.insert((x, y), tile);
    }
}

/// Deterministic integer hash producing a value in `0.0..1.0`.
///
/// Same mixing style as the storyteller's `pseudo_random` in `events.rs`:
/// SplitMix64-style avalanche so nearby coordinates don't correlate.
fn hash2(seed: u64, x: i32, y: i32) -> f32 {
    let mut value = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);

    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^= value >> 31;

    ((value >> 11) as f64 / (1u64 << 53) as f64) as f32
}

/// Smooth 2D value noise: bilinear interpolation between hashed lattice
/// corners, eased with smoothstep so the result has no visible grid seams.
fn value_noise(seed: u64, x: f32, y: f32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();

    let ix0 = x0 as i32;
    let iy0 = y0 as i32;

    let tx = x - x0;
    let ty = y - y0;

    let n00 = hash2(seed, ix0, iy0);
    let n10 = hash2(seed, ix0 + 1, iy0);
    let n01 = hash2(seed, ix0, iy0 + 1);
    let n11 = hash2(seed, ix0 + 1, iy0 + 1);

    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);

    let nx0 = n00 + (n10 - n00) * sx;
    let nx1 = n01 + (n11 - n01) * sx;

    nx0 + (nx1 - nx0) * sy
}

/// Fractal Brownian motion: several octaves of value noise summed together
/// so terrain has both broad regions and fine variation.
fn fbm(seed: u64, x: i32, y: i32, base_frequency: f32, octaves: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = base_frequency;
    let mut max_amplitude = 0.0;

    for octave in 0..octaves {
        let octave_seed = seed.wrapping_add(octave as u64 * 0x1000_0000_1);
        total += value_noise(octave_seed, x as f32 * frequency, y as f32 * frequency) * amplitude;
        max_amplitude += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }

    total / max_amplitude
}

/// Derive the tile type for a single world coordinate from noise.
///
/// Two independent low-frequency fields create broad forest and rocky
/// patches; a higher-frequency hash scatters individual trees/rocks within
/// those patches instead of filling them solid.
pub fn generate_tile(seed: u64, x: i32, y: i32) -> Tile {
    // Always keep the spawn area clear so a new run never starts blocked in.
    if x.abs() <= 2 && y.abs() <= 2 {
        return Tile::Grass;
    }

    let forest = fbm(seed, x, y, 0.07, 3);
    let rocky = fbm(seed ^ 0xA5A5_A5A5_A5A5_A5A5, x, y, 0.05, 2);
    let detail = hash2(seed ^ 0x1234_5678_9ABC_DEF0, x, y);

    if rocky > 0.60 && detail > 0.45 {
        Tile::Rock
    } else if forest > 0.56 && detail > 0.35 {
        Tile::Tree
    } else {
        Tile::Grass
    }
}

/// Circle-vs-tile collision query ("hitbox" check).
///
/// Treats every non-walkable tile as a solid 1x1 square and tests it
/// against a circle of the given radius centered at `position` (in tile
/// units). Used by both the player and enemies so no entity can walk
/// through trees, rocks, or walls.
pub fn is_position_blocked(map: &MapData, position: Vec2, radius: f32) -> bool {
    let min_x = (position.x - radius).floor() as i32;
    let max_x = (position.x + radius).floor() as i32;
    let min_y = (position.y - radius).floor() as i32;
    let max_y = (position.y + radius).floor() as i32;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if map.get(x, y).is_walkable() {
                continue;
            }

            let closest_x = position.x.clamp(x as f32, x as f32 + 1.0);
            let closest_y = position.y.clamp(y as f32, y as f32 + 1.0);

            let dx = position.x - closest_x;
            let dy = position.y - closest_y;

            if dx * dx + dy * dy < radius * radius {
                return true;
            }
        }
    }

    false
}

/// Cached handles for all world textures.
#[derive(Resource, Clone)]
pub struct GameTextures {
    pub grass: Handle<Image>,
    pub tree: Handle<Image>,
    pub rock: Handle<Image>,
    pub wall: Handle<Image>,
    pub bed: Handle<Image>,
    pub pawn: Handle<Image>,
}

impl GameTextures {
    pub fn texture_for(&self, tile: Tile) -> Handle<Image> {
        match tile {
            Tile::Grass => self.grass.clone(),
            Tile::Tree => self.tree.clone(),
            Tile::Rock => self.rock.clone(),
            Tile::Wall => self.wall.clone(),
            Tile::Bed => self.bed.clone(),
        }
    }
}

/// Identifies a terrain sprite by its logical tile coordinate.
#[derive(Component, Debug, Clone, Copy)]
pub struct TileVisual {
    pub x: i32,
    pub y: i32,
}

/// Permanent grass base for a map tile.
#[derive(Component)]
struct TileBaseVisual;

/// Optional non-grass content layered on top of a tile base.
#[derive(Component)]
struct TileContentVisual;

/// Highlight sprite placed over the currently hovered tile.
#[derive(Component)]
pub struct TileHighlight;

/// Current mouse-selected/hovered tile.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct HoveredTile {
    pub grid: Option<(i32, i32)>,
}

/// Tracks which chunks currently have spawned entities, and which entities
/// belong to each, so a chunk can be despawned as a unit when it scrolls
/// out of view.
#[derive(Resource, Default)]
pub struct LoadedChunks {
    pub chunks: HashMap<IVec2, Vec<Entity>>,
}

/// Convert grid coordinates into isometric world coordinates.
///
/// screen_x = (x - y) * TILE_WIDTH / 2
/// screen_y = (x + y) * TILE_HEIGHT / 2
///
/// Takes floating point coordinates so entities can occupy any position,
/// not just tile centers - this is what makes free, continuous movement
/// possible while still rendering on the isometric grid.
pub fn grid_to_screen(x: f32, y: f32) -> Vec2 {
    Vec2::new((x - y) * (TILE_WIDTH / 2.0), (x + y) * (TILE_HEIGHT / 2.0))
}

/// Inverse of [`grid_to_screen`], returned as continuous grid coordinates.
///
/// Because the projection is linear, the inverse is:
///
/// a = screen_x / (TILE_WIDTH / 2)
/// b = screen_y / (TILE_HEIGHT / 2)
/// x = (a + b) / 2
/// y = (b - a) / 2
pub fn screen_to_grid_f(screen: Vec2) -> Vec2 {
    let a = screen.x / (TILE_WIDTH / 2.0);
    let b = screen.y / (TILE_HEIGHT / 2.0);

    Vec2::new((a + b) / 2.0, (b - a) / 2.0)
}

/// Same as [`screen_to_grid_f`] but rounded to the nearest tile - used for
/// tile lookups (mouse picking, adjacency checks) rather than continuous
/// entity positions.
pub fn screen_to_grid(screen: Vec2) -> (i32, i32) {
    let grid = screen_to_grid_f(screen);

    (grid.x.round() as i32, grid.y.round() as i32)
}

/// Requested depth convention.
///
/// Larger Z is rendered in front of smaller Z.
///
/// Therefore the tile farther toward +x/+y receives a smaller Z. Takes
/// continuous coordinates so smoothly-moving entities get a smoothly
/// changing depth instead of popping between layers at tile boundaries.
pub fn depth_z(x: f32, y: f32, layer_offset: f32) -> f32 {
    -(x + y) + layer_offset
}

fn setup_map(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        Camera2d,
        Transform::from_xyz(0.0, 0.0, 0.0),
        Projection::Orthographic(OrthographicProjection {
            scale: 0.5,
            ..OrthographicProjection::default_2d()
        }),
    ));

    let textures = GameTextures {
        grass: asset_server.load("textures/grass.png"),
        tree: asset_server.load("textures/tree.png"),
        rock: asset_server.load("textures/rock.png"),
        wall: asset_server.load("textures/wall.png"),
        bed: asset_server.load("textures/bed.png"),
        pawn: asset_server.load("textures/pawn.png"),
    };

    commands.spawn((
        Sprite::from_color(
            Color::srgba(1.0, 1.0, 0.0, 0.35),
            Vec2::new(TILE_WIDTH, TILE_HEIGHT),
        ),
        Transform::from_xyz(0.0, 0.0, Z_LAYER_HIGHLIGHT),
        Visibility::Hidden,
        TileHighlight,
    ));

    commands.insert_resource(textures);
}

/// Follow the player smoothly through the open world.
///
/// Framerate-independent exponential smoothing: the camera always closes
/// the same *fraction* of the remaining distance per second, regardless of
/// how long each frame takes.
fn camera_follow(
    time: Res<Time>,
    pawn_q: Query<&Transform, (With<PlayerPawn>, Without<Camera2d>)>,
    mut camera_q: Query<&mut Transform, With<Camera2d>>,
) {
    let Ok(pawn_transform) = pawn_q.single() else {
        return;
    };

    let Ok(mut camera_transform) = camera_q.single_mut() else {
        return;
    };

    let target = Vec3::new(
        pawn_transform.translation.x,
        pawn_transform.translation.y,
        camera_transform.translation.z,
    );

    let smoothing = 1.0 - (-CAMERA_FOLLOW_RATE * time.delta_secs()).exp();

    camera_transform.translation = camera_transform.translation.lerp(target, smoothing);
}

/// Work out which chunks are (or should count as) currently visible, by
/// projecting the four screen corners back into grid space.
///
/// Because the isometric projection skews a rectangle into a parallelogram,
/// this returns the bounding box of that parallelogram - a safe superset of
/// what's actually on screen, padded by [`VISIBLE_TILE_MARGIN`].
fn compute_visible_chunk_range(
    windows: &Query<&Window, With<PrimaryWindow>>,
    camera_q: &Query<(&Camera, &GlobalTransform)>,
) -> Option<(IVec2, IVec2)> {
    let window = windows.single().ok()?;
    let (camera, camera_transform) = camera_q.single().ok()?;

    let width = window.width();
    let height = window.height();

    let corners = [
        Vec2::new(0.0, 0.0),
        Vec2::new(width, 0.0),
        Vec2::new(0.0, height),
        Vec2::new(width, height),
    ];

    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;

    let mut found_any = false;

    for corner in corners {
        let Ok(world) = camera.viewport_to_world_2d(camera_transform, corner) else {
            continue;
        };

        let grid = screen_to_grid_f(world);

        min_x = min_x.min(grid.x);
        max_x = max_x.max(grid.x);
        min_y = min_y.min(grid.y);
        max_y = max_y.max(grid.y);

        found_any = true;
    }

    if !found_any {
        return None;
    }

    let min_chunk = IVec2::new(
        ((min_x - VISIBLE_TILE_MARGIN) / CHUNK_SIZE as f32).floor() as i32,
        ((min_y - VISIBLE_TILE_MARGIN) / CHUNK_SIZE as f32).floor() as i32,
    );

    let max_chunk = IVec2::new(
        ((max_x + VISIBLE_TILE_MARGIN) / CHUNK_SIZE as f32).floor() as i32,
        ((max_y + VISIBLE_TILE_MARGIN) / CHUNK_SIZE as f32).floor() as i32,
    );

    Some((min_chunk, max_chunk))
}

/// Spawn every tile entity for one chunk and return their entity ids so
/// they can be despawned together later.
fn spawn_chunk(
    commands: &mut Commands,
    map: &MapData,
    textures: &GameTextures,
    chunk: IVec2,
) -> Vec<Entity> {
    let mut entities = Vec::new();

    let base_x = chunk.x * CHUNK_SIZE;
    let base_y = chunk.y * CHUNK_SIZE;

    for local_y in 0..CHUNK_SIZE {
        for local_x in 0..CHUNK_SIZE {
            let x = base_x + local_x;
            let y = base_y + local_y;

            let tile = map.get(x, y);
            let screen = grid_to_screen(x as f32, y as f32);

            // Every tile receives a permanent grass base. Non-grass terrain
            // is drawn as a separate content layer, so harvesting a tree
            // just despawns the content sprite and leaves grass showing.
            let base = commands
                .spawn((
                    Sprite::from_image(textures.grass.clone()),
                    Transform::from_xyz(
                        screen.x,
                        screen.y,
                        depth_z(x as f32, y as f32, Z_LAYER_TERRAIN),
                    ),
                    Visibility::Visible,
                    TileVisual { x, y },
                    TileBaseVisual,
                ))
                .id();
            entities.push(base);

            if tile != Tile::Grass {
                let content = commands
                    .spawn((
                        Sprite::from_image(textures.texture_for(tile)),
                        Transform::from_xyz(
                            screen.x,
                            screen.y,
                            depth_z(x as f32, y as f32, Z_LAYER_STRUCTURE),
                        ),
                        Visibility::Visible,
                        TileVisual { x, y },
                        TileContentVisual,
                    ))
                    .id();
                entities.push(content);
            }
        }
    }

    entities
}

/// Load chunks that just became visible and unload ones that scrolled far
/// enough out of view. This is what keeps an open, effectively infinite
/// world cheap to render: only nearby terrain ever exists as entities.
fn stream_chunks(
    mut commands: Commands,
    map: Res<MapData>,
    textures: Option<Res<GameTextures>>,
    mut loaded: ResMut<LoadedChunks>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
) {
    let Some(textures) = textures else {
        return;
    };

    let Some((min_chunk, max_chunk)) = compute_visible_chunk_range(&windows, &camera_q) else {
        return;
    };

    let unload_min = min_chunk - IVec2::splat(CHUNK_UNLOAD_MARGIN_CHUNKS);
    let unload_max = max_chunk + IVec2::splat(CHUNK_UNLOAD_MARGIN_CHUNKS);

    let to_unload: Vec<IVec2> = loaded
        .chunks
        .keys()
        .filter(|chunk| {
            chunk.x < unload_min.x
                || chunk.x > unload_max.x
                || chunk.y < unload_min.y
                || chunk.y > unload_max.y
        })
        .copied()
        .collect();

    for chunk in to_unload {
        if let Some(entities) = loaded.chunks.remove(&chunk) {
            for entity in entities {
                commands.entity(entity).try_despawn();
            }
        }
    }

    for chunk_y in min_chunk.y..=max_chunk.y {
        for chunk_x in min_chunk.x..=max_chunk.x {
            let coord = IVec2::new(chunk_x, chunk_y);

            if loaded.chunks.contains_key(&coord) {
                continue;
            }

            let entities = spawn_chunk(&mut commands, &map, &textures, coord);
            loaded.chunks.insert(coord, entities);
        }
    }
}

fn mouse_picking(
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    mut hovered: ResMut<HoveredTile>,
    mut highlight_q: Query<(&mut Transform, &mut Visibility), With<TileHighlight>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };

    let Ok((camera, camera_transform)) = camera_q.single() else {
        return;
    };

    let Some(cursor_position) = window.cursor_position() else {
        hovered.grid = None;

        if let Ok((_, mut visibility)) = highlight_q.single_mut() {
            *visibility = Visibility::Hidden;
        }

        return;
    };

    let Ok(world_position) = camera.viewport_to_world_2d(camera_transform, cursor_position) else {
        return;
    };

    let (x, y) = screen_to_grid(world_position);

    hovered.grid = Some((x, y));

    if let Ok((mut transform, mut visibility)) = highlight_q.single_mut() {
        let position = grid_to_screen(x as f32, y as f32);

        transform.translation.x = position.x;
        transform.translation.y = position.y;

        *visibility = Visibility::Visible;
    }
}

/// Update terrain sprite textures after a tile mutation.
fn on_tile_changed(
    mut commands: Commands,
    mut events: MessageReader<TileChangedEvent>,
    textures: Option<Res<GameTextures>>,
    mut tile_q: Query<(Entity, &TileVisual, &mut Sprite, &mut Transform), With<TileContentVisual>>,
) {
    let Some(textures) = textures else {
        return;
    };

    for event in events.read() {
        let mut content_entity = None;

        for (entity, visual, mut sprite, mut transform) in tile_q.iter_mut() {
            if visual.x != event.x || visual.y != event.y {
                continue;
            }

            content_entity = Some(entity);

            if event.new_tile == Tile::Grass {
                commands.entity(entity).try_despawn();
            } else {
                sprite.image = textures.texture_for(event.new_tile);
                transform.translation.z =
                    depth_z(event.x as f32, event.y as f32, Z_LAYER_STRUCTURE);
            }

            break;
        }

        if content_entity.is_none() && event.new_tile != Tile::Grass {
            let screen = grid_to_screen(event.x as f32, event.y as f32);

            commands.spawn((
                Sprite::from_image(textures.texture_for(event.new_tile)),
                Transform::from_xyz(
                    screen.x,
                    screen.y,
                    depth_z(event.x as f32, event.y as f32, Z_LAYER_STRUCTURE),
                ),
                Visibility::Visible,
                TileVisual {
                    x: event.x,
                    y: event.y,
                },
                TileContentVisual,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_tiles_are_not_walkable() {
        assert!(Tile::Grass.is_walkable());
        assert!(Tile::Bed.is_walkable());
        assert!(!Tile::Tree.is_walkable());
        assert!(!Tile::Rock.is_walkable());
        assert!(!Tile::Wall.is_walkable());
    }

    #[test]
    fn isometric_projection_round_trips_grid_centres() {
        for y in -20..20 {
            for x in -20..20 {
                let screen = grid_to_screen(x as f32, y as f32);
                assert_eq!(screen_to_grid(screen), (x, y));
            }
        }
    }

    #[test]
    fn spawn_area_is_always_walkable() {
        for seed in [0u64, 1, 42, u64::MAX] {
            for y in -2..=2 {
                for x in -2..=2 {
                    assert!(generate_tile(seed, x, y).is_walkable());
                }
            }
        }
    }

    #[test]
    fn generate_tile_is_deterministic() {
        let seed = 123_456_789;

        for (x, y) in [(10, 10), (-5, 30), (100, -100)] {
            assert_eq!(generate_tile(seed, x, y), generate_tile(seed, x, y));
        }
    }

    #[test]
    fn is_position_blocked_detects_solid_tile() {
        let mut map = MapData {
            seed: 0,
            overrides: HashMap::new(),
        };

        map.set(5, 5, Tile::Wall);

        assert!(is_position_blocked(&map, Vec2::new(5.5, 5.5), 0.1));
        assert!(!is_position_blocked(&map, Vec2::new(20.0, 20.0), 0.3));
    }
}

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapData>()
            .init_resource::<HoveredTile>()
            .init_resource::<LoadedChunks>()
            .add_systems(Startup, setup_map)
            .add_systems(
                Update,
                (camera_follow, stream_chunks, mouse_picking, on_tile_changed),
            );
    }
}
