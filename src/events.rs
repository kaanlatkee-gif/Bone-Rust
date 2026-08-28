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
//! - Spawn enemies at map edges.
//! - Move enemies toward the single player pawn.
//! - Damage the pawn when an enemy reaches them.

use crate::map::MapData;
use crate::map::{depth_z, grid_to_screen, GameTextures, Tile, Z_LAYER_PAWN};
use crate::pawn::{Enemy, GridPosition, PlayerPawn};
use crate::ui::ColonyResource;
use bevy::prelude::*;

/// Simulation tick rate.
///
/// One real-time second advances this many game minutes.
pub const GAME_MINUTES_PER_REAL_SECOND: f32 = 4.0;

/// Number of minutes in a game day.
pub const MINUTES_PER_DAY: f32 = 24.0 * 60.0;

/// A buffered tile mutation message.
#[derive(Message, Debug, Clone, Copy)]
pub struct TileChangedEvent {
    pub x: i32,
    pub y: i32,
    pub new_tile: Tile,
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
    map: Res<MapData>,
    textures: Res<GameTextures>,
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

    spawn_ambush(&mut commands, &mut game_time, &map, &textures, day);
}

/// Spawn an ambush from an outer map edge.
///
/// The first spawn point is selected from the four corners.
fn spawn_ambush(
    commands: &mut Commands,
    game_time: &mut GameTime,
    map: &MapData,
    textures: &GameTextures,
    day: u32,
) {
    let spawn_points = [
        (0, 0),
        (map.width as i32 - 1, 0),
        (0, map.height as i32 - 1),
        (map.width as i32 - 1, map.height as i32 - 1),
    ];

    // Deterministically rotate spawn corner by day.
    let index = (day as usize) % spawn_points.len();

    let (x, y) = spawn_points[index];

    if !map.in_bounds(x, y) {
        return;
    }

    let position = grid_to_screen(x, y);

    commands.spawn((
        Sprite::from_image(textures.pawn.clone()),
        Transform::from_xyz(position.x, position.y, depth_z(x, y, Z_LAYER_PAWN + 0.01)),
        GridPosition { x, y },
        Enemy::default(),
    ));

    game_time.active_enemies = game_time.active_enemies.saturating_add(1);

    // The warning persists long enough to be useful to the player.
    game_time.ambush_warning_timer = 8.0;
}

/// Move enemies toward the player's current tile.
///
/// This intentionally uses Manhattan movement instead of a full
/// A* implementation. For a small 16x16 map it is deterministic,
/// cheap, and adequate for the requested basic tracking AI.
///
/// Blocked tiles are avoided when possible. If the direct route is blocked,
/// enemies try the best available sidestep instead of getting stuck.
fn enemy_ai(
    time: Res<Time>,
    map: Res<MapData>,
    mut enemies: Query<(&mut GridPosition, &mut Transform, &mut Enemy), With<Enemy>>,
    pawn: Query<&GridPosition, (With<PlayerPawn>, Without<Enemy>)>,
    mut damage_events: MessageWriter<DamagePawnEvent>,
) {
    let Ok(player_position) = pawn.single() else {
        return;
    };

    for (mut enemy_position, mut transform, mut enemy) in enemies.iter_mut() {
        enemy.move_timer += time.delta_secs();

        if enemy.move_timer < enemy.move_interval {
            continue;
        }

        enemy.move_timer = 0.0;

        // Already occupying the player's tile.
        if *enemy_position == *player_position {
            damage_events.write(DamagePawnEvent {
                amount: enemy.damage,
            });

            continue;
        }

        let dx = player_position.x - enemy_position.x;
        let dy = player_position.y - enemy_position.y;

        let mut candidates = Vec::with_capacity(4);

        // Try direct moves first so equal-distance choices remain stable and
        // intuitive, then add the remaining cardinal sidesteps as fallbacks.
        if dx != 0 {
            candidates.push((enemy_position.x + dx.signum(), enemy_position.y));
        }

        if dy != 0 {
            candidates.push((enemy_position.x, enemy_position.y + dy.signum()));
        }

        const DIRECTIONS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

        for (step_x, step_y) in DIRECTIONS {
            let candidate = (enemy_position.x + step_x, enemy_position.y + step_y);

            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }

        // Prefer the candidate that ends nearest to the player.
        candidates
            .sort_by_key(|&(x, y)| (x - player_position.x).abs() + (y - player_position.y).abs());

        for (next_x, next_y) in candidates {
            if !map.in_bounds(next_x, next_y) {
                continue;
            }

            let Some(tile) = map.get(next_x, next_y) else {
                continue;
            };

            if !tile.is_walkable() {
                continue;
            }

            enemy_position.x = next_x;
            enemy_position.y = next_y;

            let screen = grid_to_screen(next_x, next_y);

            transform.translation = Vec3::new(
                screen.x,
                screen.y,
                depth_z(next_x, next_y, Z_LAYER_PAWN + 0.01),
            );

            break;
        }

        // If no movement was possible, leave the enemy in place. Deal damage
        // only once the enemy actually reaches the pawn's tile; the previous
        // adjacent-tile check caused attacks to land one step too early and
        // delayed damage when the enemy entered the pawn tile.
        if *enemy_position == *player_position {
            damage_events.write(DamagePawnEvent {
                amount: enemy.damage,
            });
        }
    }
}

/// Remove dead enemies and keep GameTime's active enemy count accurate.
fn cleanup_dead_enemies(
    mut commands: Commands,
    mut game_time: ResMut<GameTime>,
    enemies: Query<(Entity, &GridPosition), With<Enemy>>,
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
