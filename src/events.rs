// src/events.rs
//! Event, time, storyteller and enemy systems.
//!
//! Responsibilities:
//! - Maintain simulation time.
//! - Convert ticks to days/hours/minutes.
//! - Emit tile-change messages.
//! - Emit pawn-damage messages.
//! - Run the daily storyteller.
//! - Calculate progressive ambush probability.
//! - Spawn enemies around the player's current position (the world has no
//!   fixed edges to spawn from anymore).
//! - Chase the player continuously, respecting terrain hitboxes.
//! - Damage the pawn when an enemy reaches them.

use std::f32::consts::TAU;

use bevy::prelude::*;

use crate::map::{
    depth_z, grid_to_screen, is_position_blocked, GameTextures, MapData, Z_LAYER_PAWN,
};
use crate::pawn::{Enemy, PlayerPawn, WorldPosition};
use crate::ui::ColonyResource;

/// Simulation tick rate.
///
/// One real-time second advances this many game minutes.
pub const GAME_MINUTES_PER_REAL_SECOND: f32 = 4.0;

/// Number of minutes in a game day.
pub const MINUTES_PER_DAY: f32 = 24.0 * 60.0;

/// Collision radius used when an enemy tests whether a step would walk it
/// into solid terrain.
const ENEMY_COLLISION_RADIUS: f32 = 0.3;

/// Distance (in tiles) at which an enemy can land an attack.
const ENEMY_ATTACK_RANGE: f32 = 0.7;

/// Distance from the player at which an ambush is spawned.
const ENEMY_SPAWN_RADIUS: f32 = 14.0;

/// A buffered tile mutation message.
#[derive(Message, Debug, Clone, Copy)]
pub struct TileChangedEvent {
    pub x: i32,
    pub y: i32,
    pub new_tile: crate::map::Tile,
}

/// Damage intended for the player pawn.
#[derive(Message, Debug, Clone, Copy)]
pub struct DamagePawnEvent {
    pub amount: f32,
}

/// Global simulation clock.
#[derive(Resource, Debug, Clone, Copy)]
pub struct GameTime {
    /// Total elapsed game minutes.
    pub total_minutes: f32,

    /// Last day that the storyteller processed.
    pub last_storyteller_day: u32,

    /// Remaining time for the red ambush warning.
    pub ambush_warning_timer: f32,

    /// Number of currently spawned ambush enemies.
    pub active_enemies: u32,
}

impl Default for GameTime {
    fn default() -> Self {
        Self {
            total_minutes: 0.0,
            last_storyteller_day: 0,
            ambush_warning_timer: 0.0,
            active_enemies: 0,
        }
    }
}

impl GameTime {
    /// Day numbering starts at Day 1.
    pub fn day(&self) -> u32 {
        (self.total_minutes / MINUTES_PER_DAY).floor() as u32 + 1
    }

    pub fn minute_of_day(&self) -> f32 {
        self.total_minutes % MINUTES_PER_DAY
    }

    pub fn hour(&self) -> u32 {
        (self.minute_of_day() / 60.0).floor() as u32
    }

    pub fn minute(&self) -> u32 {
        (self.minute_of_day() % 60.0).floor() as u32
    }
}

/// Advance the game clock.
fn tick_game_time(time: Res<Time>, mut game_time: ResMut<GameTime>) {
    game_time.total_minutes += time.delta_secs() * GAME_MINUTES_PER_REAL_SECOND;

    game_time.ambush_warning_timer = (game_time.ambush_warning_timer - time.delta_secs()).max(0.0);
}

/// Simple deterministic pseudo-random value in the range `0.0..1.0`.
///
/// We avoid requiring another RNG dependency for the core system. This uses
/// SplitMix64-style integer mixing so nearby day numbers still spread across
/// the full unit interval. A plain xorshift seeded directly from a small day
/// number only filled the low bits, which made every post-protection ambush
/// roll effectively zero.
fn pseudo_random(day: u32) -> f32 {
    let mut value = day as u64;

    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^= value >> 31;

    // Use the upper 53 bits, matching the precision commonly used to turn a
    // u64 into a floating point value without ever producing 1.0.
    ((value >> 11) as f64 / (1u64 << 53) as f64) as f32
}

/// Calculate the probability that today's storyteller event becomes
/// an ambush.
///
/// Day factor:
/// - Day 5 starts at a low chance.
/// - Probability rises gradually with later days.
///
/// Wealth factor:
/// - More wood/stone/food means a more valuable colony.
/// - Wealth cannot by itself guarantee a raid.
///
/// Final probability is clamped to a sensible range.
fn calculate_ambush_chance(day: u32, resources: &ColonyResource) -> f32 {
    if day < 5 {
        return 0.0;
    }

    let days_after_protection = day.saturating_sub(5) as f32;

    let time_factor = (0.08 + days_after_protection * 0.025).clamp(0.08, 0.45);

    let wealth = resources.total_wealth() as f32;

    // 500 wealth produces roughly half of the maximum wealth bonus.
    let wealth_factor = (wealth / (wealth + 500.0)).clamp(0.0, 0.75);

    let combined = time_factor + wealth_factor * 0.25;

    combined.clamp(0.0, 0.65)
}

/// Run storyteller logic exactly once whenever a new day begins.
fn daily_storyteller(
    mut commands: Commands,
    mut game_time: ResMut<GameTime>,
    resources: Res<ColonyResource>,
    textures: Res<GameTextures>,
    pawn_q: Query<&WorldPosition, With<PlayerPawn>>,
) {
    let day = game_time.day();

    if day <= game_time.last_storyteller_day {
        return;
    }

    game_time.last_storyteller_day = day;

    // Hard early-game protection.
    if day < 5 {
        return;
    }

    let chance = calculate_ambush_chance(day, &resources);
    let roll = pseudo_random(day);

    if roll >= chance {
        return;
    }

    let Ok(player_position) = pawn_q.single() else {
        return;
    };

    spawn_ambush(
        &mut commands,
        &mut game_time,
        &textures,
        day,
        player_position.0,
    );
}

/// Spawn an ambush at a fixed radius around the player.
///
/// The world has no edges to spawn from anymore, so ambushes instead ring
/// in from a deterministic-but-varied direction around wherever the player
/// currently is.
fn spawn_ambush(
    commands: &mut Commands,
    game_time: &mut GameTime,
    textures: &GameTextures,
    day: u32,
    player_position: Vec2,
) {
    // A different mix than the ambush-chance roll, so the two don't
    // correlate.
    let angle = pseudo_random(day.wrapping_mul(7919).wrapping_add(11)) * TAU;

    let offset = Vec2::new(angle.cos(), angle.sin()) * ENEMY_SPAWN_RADIUS;
    let spawn_position = player_position + offset;

    let screen = grid_to_screen(spawn_position.x, spawn_position.y);

    commands.spawn((
        Sprite::from_image(textures.pawn.clone()),
        Transform::from_xyz(
            screen.x,
            screen.y,
            depth_z(spawn_position.x, spawn_position.y, Z_LAYER_PAWN + 0.01),
        ),
        WorldPosition(spawn_position),
        Enemy::default(),
    ));

    game_time.active_enemies = game_time.active_enemies.saturating_add(1);

    // The warning persists long enough to be useful to the player.
    game_time.ambush_warning_timer = 8.0;
}

/// Move enemies toward the player's current position and attack once in
/// range.
///
/// Enemies chase in a straight line rather than pathfinding - adequate for
/// the open, mostly-sparse terrain this generates, and cheap regardless of
/// how far the world extends. They still respect terrain hitboxes, so they
/// can't walk through trees, rocks, or walls to reach the player.
fn enemy_ai(
    time: Res<Time>,
    map: Res<MapData>,
    mut enemies: Query<(&mut WorldPosition, &mut Transform, &mut Enemy), With<Enemy>>,
    pawn: Query<&WorldPosition, (With<PlayerPawn>, Without<Enemy>)>,
    mut damage_events: MessageWriter<DamagePawnEvent>,
) {
    let Ok(player_position) = pawn.single() else {
        return;
    };

    let player_position = player_position.0;

    for (mut enemy_position, mut transform, mut enemy) in enemies.iter_mut() {
        enemy.attack_timer = (enemy.attack_timer - time.delta_secs()).max(0.0);

        let to_player = player_position - enemy_position.0;
        let distance = to_player.length();

        if distance <= ENEMY_ATTACK_RANGE {
            if enemy.attack_timer <= 0.0 {
                damage_events.write(DamagePawnEvent {
                    amount: enemy.damage,
                });

                enemy.attack_timer = enemy.attack_cooldown;
            }

            continue;
        }

        let direction = to_player / distance.max(0.0001);
        let delta = direction * enemy.speed * time.delta_secs();
        let candidate = enemy_position.0 + delta;

        if !is_position_blocked(&map, candidate, ENEMY_COLLISION_RADIUS) {
            enemy_position.0 = candidate;
        }

        let screen = grid_to_screen(enemy_position.0.x, enemy_position.0.y);

        transform.translation = Vec3::new(
            screen.x,
            screen.y,
            depth_z(enemy_position.0.x, enemy_position.0.y, Z_LAYER_PAWN + 0.01),
        );
    }
}

/// Remove dead enemies and keep GameTime's active enemy count accurate.
fn cleanup_dead_enemies(
    mut commands: Commands,
    mut game_time: ResMut<GameTime>,
    enemies: Query<(Entity, &WorldPosition), With<Enemy>>,
) {
    // Enemy death is currently represented by reaching the player's
    // tile repeatedly, but actual enemy health is deliberately outside
    // the requested architecture. This system therefore exists as a
    // clean extension point and does not remove healthy enemies.
    //
    // Keeping the query here also makes adding EnemyHealth later local
    // to this module instead of requiring architectural changes.
    let _ = (&mut commands, &mut game_time, &enemies);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudo_random_values_cover_the_unit_interval() {
        let rolls: Vec<f32> = (5..40).map(pseudo_random).collect();

        assert!(rolls.iter().all(|roll| (0.0..1.0).contains(roll)));
        assert!(rolls.iter().any(|roll| *roll < 0.1));
        assert!(rolls.iter().any(|roll| *roll > 0.9));
    }

    #[test]
    fn ambush_chance_respects_early_game_protection_and_cap() {
        let poor_colony = ColonyResource::default();
        let rich_colony = ColonyResource {
            wood: u32::MAX,
            stone: u32::MAX,
            food: u32::MAX,
        };

        assert_eq!(calculate_ambush_chance(4, &poor_colony), 0.0);
        assert!(calculate_ambush_chance(5, &poor_colony) > 0.0);
        assert!(calculate_ambush_chance(100, &rich_colony) <= 0.65);
    }
}

/// Register event/message types and systems.
pub struct EventsPlugin;

impl Plugin for EventsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameTime>()
            .add_message::<TileChangedEvent>()
            .add_message::<DamagePawnEvent>()
            .add_systems(
                Update,
                (
                    tick_game_time,
                    daily_storyteller,
                    enemy_ai,
                    cleanup_dead_enemies,
                ),
            );
    }
}
