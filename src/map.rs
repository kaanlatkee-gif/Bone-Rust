// src/map.rs
//! Map module.
//!
//! Responsibilities:
//! - Store the tile grid.
//! - Provide isometric projection helpers.
//! - Load and cache map textures.
//! - Spawn terrain.
//! - Maintain terrain depth ordering.
//! - Convert mouse coordinates back to grid coordinates.
//! - Highlight the currently hovered tile.
//! - React to tile mutation events.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::events::TileChangedEvent;

pub const MAP_SIZE: usize = 16;

pub const TILE_WIDTH: f32 = 64.0;
pub const TILE_HEIGHT: f32 = 32.0;

pub const Z_LAYER_TERRAIN: f32 = 0.0;
pub const Z_LAYER_STRUCTURE: f32 = 0.01;
pub const Z_LAYER_PAWN: f32 = 0.02;
pub const Z_LAYER_HIGHLIGHT: f32 = 100.0;

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
        !matches!(self, Tile::Wall)
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

/// Row-major storage: tiles[y][x].
#[derive(Resource, Debug, Clone)]
pub struct MapData {
    pub tiles: Vec<Vec<Tile>>,
    pub width: usize,
    pub height: usize,
}

impl Default for MapData {
    fn default() -> Self {
        let mut tiles = vec![vec![Tile::Grass; MAP_SIZE]; MAP_SIZE];

        // Deterministic starter map.
        for y in 0..MAP_SIZE {
            for x in 0..MAP_SIZE {
                let border = x == 0
                    || y == 0
                    || x == MAP_SIZE - 1
                    || y == MAP_SIZE - 1;

                if border {
                    continue;
                }

                if (x + y * 3) % 11 == 0 {
                    tiles[y][x] = Tile::Tree;
                } else if (x * 2 + y) % 13 == 0 {
                    tiles[y][x] = Tile::Rock;
                }
            }
        }

        // Starter bed.
        tiles[2][2] = Tile::Bed;

        Self {
            tiles,
            width: MAP_SIZE,
            height: MAP_SIZE,
        }
    }
}

impl MapData {
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.width
            && (y as usize) < self.height
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Tile> {
        if !self.in_bounds(x, y) {
            return None;
        }

        Some(self.tiles[y as usize][x as usize])
    }

    pub fn set(&mut self, x: i32, y: i32, tile: Tile) {
        if self.in_bounds(x, y) {
            self.tiles[y as usize][x as usize] = tile;
        }
    }
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

/// Highlight sprite placed over the currently hovered tile.
#[derive(Component)]
pub struct TileHighlight;

/// Current mouse-selected/hovered tile.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct HoveredTile {
    pub grid: Option<(i32, i32)>,
}

/// Convert grid coordinates into isometric world coordinates.
///
/// screen_x = (x - y) * TILE_WIDTH / 2
/// screen_y = (x + y) * TILE_HEIGHT / 2
pub fn grid_to_screen(x: i32, y: i32) -> Vec2 {
    let x = x as f32;
    let y = y as f32;

    Vec2::new(
        (x - y) * (TILE_WIDTH / 2.0),
        (x + y) * (TILE_HEIGHT / 2.0),
    )
}

/// Inverse of [`grid_to_screen`].
///
/// Because the projection is linear, the inverse is:
///
/// a = screen_x / (TILE_WIDTH / 2)
/// b = screen_y / (TILE_HEIGHT / 2)
/// x = (a + b) / 2
/// y = (b - a) / 2
pub fn screen_to_grid(screen: Vec2) -> (i32, i32) {
    let a = screen.x / (TILE_WIDTH / 2.0);
    let b = screen.y / (TILE_HEIGHT / 2.0);

    let x = (a + b) / 2.0;
    let y = (b - a) / 2.0;

    (x.round() as i32, y.round() as i32)
}

/// Requested depth convention.
///
/// Larger Z is rendered in front of smaller Z.
///
/// Therefore the tile farther toward +x/+y receives a smaller Z.
pub fn depth_z(x: i32, y: i32, layer_offset: f32) -> f32 {
    -(x + y) as f32 + layer_offset
}

pub(crate) fn setup_map(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    map: Res<MapData>,
) {
    // A centered 2D camera renders the isometric world; UI uses the default camera.
    commands.spawn(Camera2d);

    let textures = GameTextures {
        grass: asset_server.load("textures/grass.png"),
        tree: asset_server.load("textures/tree.png"),
        rock: asset_server.load("textures/rock.png"),
        wall: asset_server.load("textures/wall.png"),
        bed: asset_server.load("textures/bed.png"),
        pawn: asset_server.load("textures/pawn.png"),
    };

    for y in 0..map.height {
        for x in 0..map.width {
            let x = x as i32;
            let y = y as i32;

            let tile = map.get(x, y).unwrap_or(Tile::Grass);
            let screen = grid_to_screen(x, y);

            // Every occupied terrain tile receives a grass base.
            // This prevents transparent structures from leaving holes.
            if tile != Tile::Grass {
                commands.spawn((
                    Sprite::from_image(textures.grass.clone()),
                    Transform::from_xyz(
                        screen.x,
                        screen.y,
                        depth_z(x, y, Z_LAYER_TERRAIN),
                    ),
                    TileVisual { x, y },
                ));
            }

            commands.spawn((
                Sprite::from_image(textures.texture_for(tile)),
                Transform::from_xyz(
                    screen.x,
                    screen.y,
                    depth_z(x, y, if tile == Tile::Grass {
                        Z_LAYER_TERRAIN
                    } else {
                        Z_LAYER_STRUCTURE
                    }),
                ),
                TileVisual { x, y },
            ));
        }
    }

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

fn mouse_picking(
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    map: Res<MapData>,
    mut hovered: ResMut<HoveredTile>,
    mut highlight_q: Query<
        (&mut Transform, &mut Visibility),
        With<TileHighlight>,
    >,
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

    let Ok(world_position) =
        camera.viewport_to_world_2d(camera_transform, cursor_position)
    else {
        return;
    };

    let (x, y) = screen_to_grid(world_position);

    if !map.in_bounds(x, y) {
        hovered.grid = None;

        if let Ok((_, mut visibility)) = highlight_q.single_mut() {
            *visibility = Visibility::Hidden;
        }

        return;
    }

    hovered.grid = Some((x, y));

    if let Ok((mut transform, mut visibility)) = highlight_q.single_mut() {
        let position = grid_to_screen(x, y);

        transform.translation.x = position.x;
        transform.translation.y = position.y;

        *visibility = Visibility::Visible;
    }
}

/// Update terrain sprite textures after a tile mutation.
fn on_tile_changed(
    mut events: MessageReader<TileChangedEvent>,
    textures: Option<Res<GameTextures>>,
    mut tile_q: Query<(&TileVisual, &mut Sprite, &mut Transform)>,
) {
    let Some(textures) = textures else {
        return;
    };

    for event in events.read() {
        for (visual, mut sprite, mut transform) in tile_q.iter_mut() {
            if visual.x != event.x || visual.y != event.y {
                continue;
            }

            sprite.image = textures.texture_for(event.new_tile);

            transform.translation.z = depth_z(
                event.x,
                event.y,
                if event.new_tile == Tile::Grass {
                    Z_LAYER_TERRAIN
                } else {
                    Z_LAYER_STRUCTURE
                },
            );
        }
    }
}

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapData>()
            .init_resource::<HoveredTile>()
            .add_systems(Startup, setup_map)
            .add_systems(Update, (
                mouse_picking,
                on_tile_changed,
            ));
    }
}
