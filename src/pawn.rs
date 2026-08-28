// src/pawn.rs
//! Pawn module.
//!
//! Responsibilities:
//! - Define the single player Pawn.
//! - Track health, hunger, energy and mood.
//! - Handle grid-based movement.
//! - Smoothly interpolate movement in world space.
//! - Simulate pawn needs.
//! - Harvest adjacent trees and rocks.
//! - Track whether the pawn is sleeping on a bed/raw ground.
//! - Receive enemy damage.

use bevy::prelude::*;

use crate::events::{DamagePawnEvent, TileChangedEvent};
use crate::map::{
    depth_z,
    grid_to_screen,
    MapData,
    Tile,
    Z_LAYER_PAWN,
};
use crate::ui::ColonyResource;

const NEED_DECAY_PER_SECOND: f32 = 0.5;
const STARVATION_DAMAGE_PER_SECOND: f32 = 2.0;
const LOW_NEED_MOOD_LOSS_PER_SECOND: f32 = 1.0;
const RAW_GROUND_SLEEP_MOOD_LOSS_PER_SECOND: f32 = 1.5;

const MOVE_DURATION: f32 = 0.12;

/// A grid coordinate.
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

/// Movement interpolation state.
#[derive(Component, Debug, Clone, Copy)]
pub struct PawnMovement {
    pub from: Vec3,
    pub to: Vec3,
    pub elapsed: f32,
    pub duration: f32,
}

impl PawnMovement {
    pub fn idle(position: Vec3) -> Self {
        Self {
            from: position,
            to: position,
            elapsed: MOVE_DURATION,
            duration: MOVE_DURATION,
        }
    }

    pub fn is_moving(&self) -> bool {
        self.elapsed < self.duration
    }
}

/// Marker for a hostile NPC.
#[derive(Component, Debug, Clone, Copy)]
pub struct Enemy {
    /// How often this enemy takes another step toward the player.
    pub move_interval: f32,

    /// Accumulated time since the last movement.
    pub move_timer: f32,

    /// Damage dealt upon reaching the player.
    pub damage: f32,
}

impl Default for Enemy {
    fn default() -> Self {
        Self {
            move_interval: 2.5,
            move_timer: 0.0,
            damage: 10.0,
        }
    }
}

/// Spawn the single player pawn.
fn setup_pawn(
    mut commands: Commands,
    textures: Res<crate::map::GameTextures>,
) {
    let start = GridPosition::new(3, 3);
    let screen = grid_to_screen(start.x, start.y);

    let position = Vec3::new(
        screen.x,
        screen.y,
        depth_z(start.x, start.y, Z_LAYER_PAWN),
    );

    commands.spawn((
        Sprite::from_image(textures.pawn.clone()),
        Transform::from_translation(position),
        Pawn {
            needs: PawnNeeds::default(),
        },
        PlayerPawn,
        start,
        PawnMovement::idle(position),
    ));
}

/// Convert keyboard input into one discrete grid step.
///
/// W / Up    = y - 1
/// S / Down  = y + 1
/// A / Left  = x - 1
/// D / Right = x + 1
fn pawn_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    map: Res<MapData>,
    mut pawn_q: Query<
        (
            &mut GridPosition,
            &mut PawnMovement,
            &Transform,
        ),
        With<PlayerPawn>,
    >,
) {
    let Ok((mut grid, mut movement, transform)) = pawn_q.single_mut() else {
        return;
    };

    // Do not queue arbitrary movement while the pawn is still
    // interpolating toward the previous tile.
    if movement.is_moving() {
        return;
    }

    let mut delta = IVec2::ZERO;

    if keyboard.just_pressed(KeyCode::KeyW)
        || keyboard.just_pressed(KeyCode::ArrowUp)
    {
        delta.y -= 1;
    } else if keyboard.just_pressed(KeyCode::KeyS)
        || keyboard.just_pressed(KeyCode::ArrowDown)
    {
        delta.y += 1;
    } else if keyboard.just_pressed(KeyCode::KeyA)
        || keyboard.just_pressed(KeyCode::ArrowLeft)
    {
        delta.x -= 1;
    } else if keyboard.just_pressed(KeyCode::KeyD)
        || keyboard.just_pressed(KeyCode::ArrowRight)
    {
        delta.x += 1;
    }

    if delta == IVec2::ZERO {
        return;
    }

    let target_x = grid.x + delta.x;
    let target_y = grid.y + delta.y;

    if !map.in_bounds(target_x, target_y) {
        return;
    }

    let Some(tile) = map.get(target_x, target_y) else {
        return;
    };

    if !tile.is_walkable() {
        return;
    }

    grid.x = target_x;
    grid.y = target_y;

    let target = grid_to_screen(target_x, target_y);

    movement.from = transform.translation;
    movement.to = Vec3::new(
        target.x,
        target.y,
        depth_z(target_x, target_y, Z_LAYER_PAWN),
    );
    movement.elapsed = 0.0;
    movement.duration = MOVE_DURATION;
}

/// Smooth the pawn's transform toward its current grid coordinate.
fn smooth_pawn_movement(
    time: Res<Time>,
    mut pawn_q: Query<(&mut Transform, &mut PawnMovement), With<PlayerPawn>>,
) {
    let Ok((mut transform, mut movement)) = pawn_q.single_mut() else {
        return;
    };

    if !movement.is_moving() {
        transform.translation = movement.to;
        return;
    }

    movement.elapsed += time.delta_secs();

    let t = (movement.elapsed / movement.duration).clamp(0.0, 1.0);

    // Smoothstep interpolation.
    let smooth_t = t * t * (3.0 - 2.0 * t);

    transform.translation = movement.from.lerp(movement.to, smooth_t);

    if t >= 1.0 {
        transform.translation = movement.to;
        movement.elapsed = movement.duration;
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

    let current_tile = map
        .get(position.x, position.y)
        .unwrap_or(Tile::Grass);

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
    const DIRECTIONS: [(i32, i32); 4] = [
        (0, -1),
        (1, 0),
        (0, 1),
        (-1, 0),
    ];

    for (dx, dy) in DIRECTIONS {
        let x = pawn_position.x + dx;
        let y = pawn_position.y + dy;

        let Some(tile) = map.get(x, y) else {
            continue;
        };

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
        pawn.needs.health =
            (pawn.needs.health - event.amount.max(0.0)).clamp(0.0, 100.0);
    }
}

pub struct PawnPlugin;

impl Plugin for PawnPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_pawn.after(crate::map::setup_map))
            .add_systems(
                Update,
                (
                    pawn_input,
                    smooth_pawn_movement,
                    tick_needs,
                    harvest_adjacent,
                    receive_damage,
                ),
            );
    }
}
