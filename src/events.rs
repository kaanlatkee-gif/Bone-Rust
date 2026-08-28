use bevy::prelude::*;
use crate::map::{depth_z, grid_to_screen, MapData, Tile, Z_LAYER_PAWN};
use crate::pawn::{Enemy, GridPosition, PlayerPawn};
use crate::ui::ColonyResource;

pub const TICKS_PER_HOUR: f32 = 60.0;
pub const HOURS_PER_DAY: u32 = 24;

#[derive(Resource, Debug, Clone)]
pub struct GameTime {
    pub elapsed_ticks: f32,
    pub last_event_day: u32,
    pub ambush_warning_timer: f32,
    pub ambush_count: u32,
}
impl Default for GameTime { fn default() -> Self { Self { elapsed_ticks: 0.0, last_event_day: 1, ambush_warning_timer: 0.0, ambush_count: 0 } } }
impl GameTime {
    pub fn total_hours(&self) -> f32 { self.elapsed_ticks / TICKS_PER_HOUR }
    pub fn day(&self) -> u32 { (self.total_hours() / HOURS_PER_DAY as f32).floor() as u32 + 1 }
    pub fn hour(&self) -> u32 { (self.total_hours() as u32) % HOURS_PER_DAY }
    pub fn minute(&self) -> u32 { ((self.elapsed_ticks % TICKS_PER_HOUR) / TICKS_PER_HOUR * 60.0) as u32 }
}

#[derive(Message, Debug, Clone, Copy)]
pub struct TileChangedEvent { pub x: i32, pub y: i32, pub new_tile: Tile }
#[derive(Message, Debug, Clone, Copy)]
pub struct DamagePawnEvent { pub amount: f32 }

fn advance_time(time: Res<Time>, mut game: ResMut<GameTime>) {
    game.elapsed_ticks += time.delta_secs();
    game.ambush_warning_timer = (game.ambush_warning_timer - time.delta_secs()).max(0.0);
}

fn daily_storyteller(mut commands: Commands, mut game: ResMut<GameTime>, resources: Res<ColonyResource>, map: Res<MapData>) {
    let day = game.day();
    if day <= game.last_event_day || day < 5 { return; }
    game.last_event_day = day;
    let wealth = resources.total_wealth() as f32;
    let chance = (0.08 + (day - 5) as f32 * 0.025 + wealth * 0.001).min(0.75);
    // Deterministic pseudo-random roll keeps the core module dependency-free.
    let roll = ((day as u64 * 1_103_515_245 + game.ambush_count as u64 * 12_345 + 12_345) % 1000) as f32 / 1000.0;
    if roll >= chance { return; }
    game.ambush_count += 1;
    game.ambush_warning_timer = 15.0;
    let (x, y) = if game.ambush_count % 2 == 0 { (0, 0) } else { ((map.width - 1) as i32, (map.height - 1) as i32) };
    let p = grid_to_screen(x, y);
    commands.spawn((Enemy::default(), GridPosition { x, y }, Sprite::from_color(Color::srgb(0.8, 0.05, 0.05), Vec2::splat(24.0)), Transform::from_xyz(p.x, p.y, depth_z(x, y, Z_LAYER_PAWN))));
}

fn enemy_tracking(time: Res<Time>, mut enemies: Query<(&mut Enemy, &mut GridPosition, &mut Transform)>, pawn: Query<&GridPosition, With<PlayerPawn>>, mut damage: MessageWriter<DamagePawnEvent>) {
    let Ok(target) = pawn.single() else { return; };
    for (mut enemy, mut pos, mut transform) in &mut enemies {
        enemy.move_timer += time.delta_secs();
        if enemy.move_timer < enemy.move_interval { continue; }
        enemy.move_timer = 0.0;
        if pos.x == target.x && pos.y == target.y { damage.write(DamagePawnEvent { amount: enemy.damage }); continue; }
        if (target.x - pos.x).abs() >= (target.y - pos.y).abs() { pos.x += (target.x - pos.x).signum(); } else { pos.y += (target.y - pos.y).signum(); }
        let p = grid_to_screen(pos.x, pos.y);
        transform.translation = Vec3::new(p.x, p.y, depth_z(pos.x, pos.y, Z_LAYER_PAWN));
    }
}

pub struct EventsPlugin;
impl Plugin for EventsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameTime>()
            .add_message::<TileChangedEvent>()
            .add_message::<DamagePawnEvent>()
            .add_systems(Update, (advance_time, daily_storyteller, enemy_tracking));
    }
}
