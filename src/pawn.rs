// src/pawn.rs
//! Pawn module.
//!
//! Responsibilities:
//! - Define the single player Pawn.
//! - Track health, hunger, energy and mood.
//! - Handle free, continuous movement with real collision against terrain.
//! - Keep a derived grid coordinate in sync for tile-based systems.
//! - Simulate pawn needs.
//! - Harvest adjacent trees and rocks.
//! - Track whether the pawn is sleeping on a bed/raw ground.
//! - Receive enemy damage.

use bevy::prelude::*;

use crate::events::{DamagePawnEvent, TileChangedEvent};
use crate::map::{depth_z, grid_to_screen, is_position_blocked, MapData, Tile, Z_LAYER_PAWN};
use crate::ui::ColonyResource;

const NEED_DECAY_PER_SECOND: f32 = 0.5;
const STARVATION_DAMAGE_PER_SECOND: f32 = 2.0;
const LOW_NEED_MOOD_LOSS_PER_SECOND: f32 = 1.0;
const RAW_GROUND_SLEEP_MOOD_LOSS_PER_SECOND: f32 = 1.5;

/// Movement speed in tiles per second. Movement is fully continuous - no
/// grid-snapping, no cooldown between steps, no discrete tiles at all.
const PAWN_SPEED: f32 = 4.0;

/// Radius (in tile units) of the pawn's collision circle against terrain.
const PAWN_COLLISION_RADIUS: f32 = 0.3;

/// A grid coordinate - the tile an entity is currently considered to be
/// "on" for tile-based systems (harvesting, sleeping checks). Derived each
/// frame from the entity's continuous [`WorldPosition`], it is not the
/// source of truth for rendering or movement anymore.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPosition {
    pub x: i32,
    pub y: i32,
}

impl GridPosition {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn manhattan_distance(self, other: Self) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }

    pub fn adjacent_to(self, other: Self) -> bool {
        self.manhattan_distance(other) == 1
    }
}

/// Continuous world-space position, in tile units. This is the source of
/// truth for movement, rendering, and collision for any entity that moves
/// freely through the open world (the player, enemies).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WorldPosition(pub Vec2);

/// Player survival needs.
///
/// All values are kept in the range 0..=100.
#[derive(Debug, Clone, Copy)]
pub struct PawnNeeds {
    pub health: f32,
    pub hunger: f32,
    pub energy: f32,
    pub mood: f32,
}

impl Default for PawnNeeds {
    fn default() -> Self {
        Self {
            health: 100.0,
            hunger: 100.0,
            energy: 100.0,
            mood: 100.0,
        }
    }
}

impl PawnNeeds {
    pub fn clamp(&mut self) {
        self.health = self.health.clamp(0.0, 100.0);
        self.hunger = self.hunger.clamp(0.0, 100.0);
        self.energy = self.energy.clamp(0.0, 100.0);
        self.mood = self.mood.clamp(0.0, 100.0);
    }
}

/// The one controllable pawn.
///
/// Exactly one entity in the game should carry this component.
#[derive(Component, Debug, Clone, Copy)]
pub struct Pawn {
    pub needs: PawnNeeds,
}

/// Marker identifying the player-controlled pawn.
#[derive(Component)]
pub struct PlayerPawn;

/// Marker for a hostile NPC.
#[derive(Component, Debug, Clone, Copy)]
pub struct Enemy {
    /// Movement speed in tiles per second.
    pub speed: f32,

    /// Damage dealt per attack once in range.
    pub damage: f32,

    /// Minimum time between attacks.
    pub attack_cooldown: f32,

    /// Time remaining before this enemy can attack again.
    pub attack_timer: f32,
}

impl Default for Enemy {
    fn default() -> Self {
        Self {
            speed: 1.6,
            damage: 10.0,
            attack_cooldown: 1.0,
            attack_timer: 0.0,
        }
    }
}

/// Spawn the single player pawn.
fn setup_pawn(mut commands: Commands, asset_server: Res<AssetServer>) {
    let start = Vec2::new(0.0, 0.0);
    let screen = grid_to_screen(start.x, start.y);
    let position = Vec3::new(screen.x, screen.y, depth_z(start.x, start.y, Z_LAYER_PAWN));

    commands.spawn((
        Sprite::from_image(asset_server.load("textures/pawn.png")),
        Transform::from_translation(position),
        Pawn {
            needs: PawnNeeds::default(),
        },
        PlayerPawn,
        WorldPosition(start),
        GridPosition::new(0, 0),
    ));
}

/// Move the pawn freely in any direction, with no grid-snapping and no
/// cooldown between inputs.
///
/// Each axis is resolved independently against terrain collision, so the
/// pawn slides along an obstacle's edge instead of stopping dead the moment
/// either axis alone would intersect something solid.
fn pawn_movement(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    map: Res<MapData>,
    mut pawn_q: Query<(&mut WorldPosition, &mut Transform), With<PlayerPawn>>,
) {
    let Ok((mut world_position, mut transform)) = pawn_q.single_mut() else {
        return;
    };

    let mut direction = Vec2::ZERO;

    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        direction.y -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        direction.y += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) || keyboard.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) || keyboard.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }

    if direction == Vec2::ZERO {
        return;
    }

    direction = direction.normalize();

    let delta = direction * PAWN_SPEED * time.delta_secs();
    let mut position = world_position.0;

    let candidate_x = Vec2::new(position.x + delta.x, position.y);
    if !is_position_blocked(&map, candidate_x, PAWN_COLLISION_RADIUS) {
        position.x = candidate_x.x;
    }

    let candidate_y = Vec2::new(position.x, position.y + delta.y);
    if !is_position_blocked(&map, candidate_y, PAWN_COLLISION_RADIUS) {
        position.y = candidate_y.y;
    }

    world_position.0 = position;

    let screen = grid_to_screen(position.x, position.y);
    transform.translation.x = screen.x;
    transform.translation.y = screen.y;
    transform.translation.z = depth_z(position.x, position.y, Z_LAYER_PAWN);
}

/// Keep the player's [`GridPosition`] matching their continuous position,
/// for the benefit of tile-based systems (harvesting, sleeping checks).
fn sync_grid_position(mut pawn_q: Query<(&WorldPosition, &mut GridPosition), With<PlayerPawn>>) {
    let Ok((world_position, mut grid)) = pawn_q.single_mut() else {
        return;
    };

    let x = world_position.0.x.round() as i32;
    let y = world_position.0.y.round() as i32;

    if grid.x != x || grid.y != y {
        grid.x = x;
        grid.y = y;
    }
}

/// Deplete survival needs.
fn tick_needs(
    time: Res<Time>,
    map: Res<MapData>,
    mut pawn_q: Query<(&GridPosition, &mut Pawn), With<PlayerPawn>>,
) {
    let Ok((position, mut pawn)) = pawn_q.single_mut() else {
        return;
    };

    let dt = time.delta_secs();

    pawn.needs.hunger -= NEED_DECAY_PER_SECOND * dt;
    pawn.needs.energy -= NEED_DECAY_PER_SECOND * dt;

    if pawn.needs.hunger <= 0.0 {
        pawn.needs.health -= STARVATION_DAMAGE_PER_SECOND * dt;
    }

    let current_tile = map.get(position.x, position.y);

    if pawn.needs.hunger < 20.0 || pawn.needs.energy < 20.0 {
        pawn.needs.mood -= LOW_NEED_MOOD_LOSS_PER_SECOND * dt;
    }

    // "Sleeping on raw ground" is represented by being on Grass.
    // Bed tiles provide the proper sleeping surface.
    //
    // This does not automatically force sleep. It represents the
    // requested mood penalty when a sleeping pawn is on raw ground.
    if current_tile == Tile::Grass && pawn.needs.energy < 10.0 {
        pawn.needs.mood -= RAW_GROUND_SLEEP_MOOD_LOSS_PER_SECOND * dt;
    }

    pawn.needs.clamp();
}

/// Press E next to a Tree or Rock to harvest it.
fn harvest_adjacent(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut map: ResMut<MapData>,
    pawn_q: Query<&GridPosition, With<PlayerPawn>>,
    mut resources: ResMut<ColonyResource>,
    mut tile_events: MessageWriter<TileChangedEvent>,
) {
    if !keyboard.just_pressed(KeyCode::KeyE) {
        return;
    }

    let Ok(pawn_position) = pawn_q.single() else {
        return;
    };

    // Check the four cardinal neighbors.
    const DIRECTIONS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

    for (dx, dy) in DIRECTIONS {
        let x = pawn_position.x + dx;
        let y = pawn_position.y + dy;

        let tile = map.get(x, y);

        match tile {
            Tile::Tree => {
                map.set(x, y, Tile::Grass);
                resources.wood = resources.wood.saturating_add(10);

                tile_events.write(TileChangedEvent {
                    x,
                    y,
                    new_tile: Tile::Grass,
                });

                break;
            }

            Tile::Rock => {
                map.set(x, y, Tile::Grass);
                resources.stone = resources.stone.saturating_add(8);

                tile_events.write(TileChangedEvent {
                    x,
                    y,
                    new_tile: Tile::Grass,
                });

                break;
            }

            _ => {}
        }
    }
}

/// Apply combat damage to the one pawn.
fn receive_damage(
    mut events: MessageReader<DamagePawnEvent>,
    mut pawn_q: Query<&mut Pawn, With<PlayerPawn>>,
) {
    let Ok(mut pawn) = pawn_q.single_mut() else {
        return;
    };

    for event in events.read() {
        pawn.needs.health = (pawn.needs.health - event.amount.max(0.0)).clamp(0.0, 100.0);
    }
}

pub struct PawnPlugin;

impl Plugin for PawnPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_pawn).add_systems(
            Update,
            (
                pawn_movement,
                sync_grid_position,
                tick_needs,
                harvest_adjacent,
                receive_damage,
            )
                .chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_position_adjacency() {
        let a = GridPosition::new(3, 3);
        let b = GridPosition::new(3, 4);
        let c = GridPosition::new(5, 5);

        assert!(a.adjacent_to(b));
        assert!(!a.adjacent_to(c));
    }
}
