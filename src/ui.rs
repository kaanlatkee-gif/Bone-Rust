// src/ui.rs
//! UI module.
//!
//! Responsibilities:
//! - Global colony resource economy.
//! - Day/time HUD.
//! - Resource HUD.
//! - Pawn needs bars.
//! - Ambush warning display.

use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;

use crate::events::GameTime;
use crate::pawn::{Pawn, PlayerPawn};

/// Global colony economy.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ColonyResource {
    pub wood: u32,
    pub stone: u32,
    pub food: u32,
}

impl Default for ColonyResource {
    fn default() -> Self {
        Self {
            wood: 0,
            stone: 0,
            food: 0,
        }
    }
}

impl ColonyResource {
    pub fn total_wealth(&self) -> u32 {
        self.wood
            .saturating_add(self.stone)
            .saturating_add(self.food)
    }
}

#[derive(Component)]
struct DayTimeText;

#[derive(Component)]
struct ResourceText;

#[derive(Component)]
struct HealthBar;

#[derive(Component)]
struct HungerBar;

#[derive(Component)]
struct EnergyBar;

#[derive(Component)]
struct MoodBar;

#[derive(Component)]
struct AmbushWarning;

/// Root HUD node.
#[derive(Component)]
struct HudRoot;

const BAR_WIDTH: f32 = 220.0;
const BAR_HEIGHT: f32 = 18.0;

fn setup_ui(mut commands: Commands) {
    commands.spawn((
        Node {
            width: percent(100),
            height: percent(100),
            position_type: PositionType::Absolute,
            ..default()
        },
        HudRoot,
        children![
            // ---------------------------------------------------------
            // Top-left: day/time
            // ---------------------------------------------------------
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(16),
                    top: px(16),
                    padding: UiRect::all(px(10)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.65)),
                children![(
                    Text::new("Day 1  00:00"),
                    TextFont {
                        font_size: 20.0_f32.into(),
                        ..default()
                    },
                    TextColor(Color::WHITE.into()),
                    DayTimeText,
                )]
            ),
            // ---------------------------------------------------------
            // Top-right: resources
            // ---------------------------------------------------------
            (
                Node {
                    position_type: PositionType::Absolute,
                    right: px(16),
                    top: px(16),
                    padding: UiRect::all(px(10)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.65)),
                children![(
                    Text::new("Wood: 0   Stone: 0   Food: 0"),
                    TextFont {
                        font_size: 20.0_f32.into(),
                        ..default()
                    },
                    TextColor(Color::WHITE.into()),
                    ResourceText,
                )]
            ),
            // ---------------------------------------------------------
            // Bottom center: needs
            // ---------------------------------------------------------
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(50),
                    bottom: px(20),
                    width: px(BAR_WIDTH),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(6),
                    ..default()
                },
                UiTransform::from_translation(Val2::new(percent(-50), px(0)),),
                children![
                    status_bar("Health", HealthBar),
                    status_bar("Hunger", HungerBar),
                    status_bar("Energy", EnergyBar),
                    status_bar("Mood", MoodBar),
                ]
            ),
            // ---------------------------------------------------------
            // Center: ambush warning
            // ---------------------------------------------------------
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(50),
                    top: px(90),
                    ..default()
                },
                UiTransform::from_translation(Val2::new(percent(-50), px(0)),),
                children![(
                    Text::new(""),
                    TextFont {
                        font_size: 34.0_f32.into(),
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 0.1, 0.1).into()),
                    AmbushWarning,
                )]
            ),
        ],
    ));
}

/// Build a labelled progress bar.
///
/// The actual filled percentage is controlled by `update_status_bars`.
fn status_bar<T: Component>(label: &'static str, marker: T) -> impl Bundle {
    (
        Node {
            width: px(BAR_WIDTH),
            height: px(BAR_HEIGHT),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.05, 0.05, 0.9)),
        children![
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: px(0),
                    width: px(BAR_WIDTH),
                    height: px(BAR_HEIGHT),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.2, 0.8, 0.2, 0.9)),
                marker,
            ),
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(6),
                    top: px(0),
                    height: px(BAR_HEIGHT),
                    align_items: AlignItems::Center,
                    ..default()
                },
                children![(
                    Text::new(label),
                    TextFont {
                        font_size: 13.0_f32.into(),
                        ..default()
                    },
                    TextColor(Color::WHITE.into()),
                )]
            ),
        ],
    )
}

fn update_day_time(game_time: Res<GameTime>, mut query: Query<&mut Text, With<DayTimeText>>) {
    if !game_time.is_changed() {
        return;
    }

    let day = game_time.day();
    let hour = game_time.hour();
    let minute = game_time.minute();

    let Ok(mut text) = query.single_mut() else {
        return;
    };

    **text = format!("Day {}  {:02}:{:02}", day, hour, minute);
}

fn update_resources(
    resources: Res<ColonyResource>,
    mut query: Query<&mut Text, With<ResourceText>>,
) {
    if !resources.is_changed() {
        return;
    }

    let Ok(mut text) = query.single_mut() else {
        return;
    };

    **text = format!(
        "🪵 Wood: {}   🪨 Stone: {}   🍎 Food: {}",
        resources.wood, resources.stone, resources.food
    );
}

fn update_status_bars(
    pawn_q: Query<&Pawn, With<PlayerPawn>>,
    mut bars: ParamSet<(
        Query<&mut Node, With<HealthBar>>,
        Query<&mut Node, With<HungerBar>>,
        Query<&mut Node, With<EnergyBar>>,
        Query<&mut Node, With<MoodBar>>,
    )>,
) {
    let Ok(pawn) = pawn_q.single() else {
        return;
    };

    set_bar(&mut bars.p0(), pawn.needs.health);
    set_bar(&mut bars.p1(), pawn.needs.hunger);
    set_bar(&mut bars.p2(), pawn.needs.energy);
    set_bar(&mut bars.p3(), pawn.needs.mood);
}

fn set_bar<F: QueryFilter>(query: &mut Query<&mut Node, F>, value: f32) {
    let Ok(mut node) = query.single_mut() else {
        return;
    };

    node.width = px(BAR_WIDTH * (value / 100.0).clamp(0.0, 1.0));
}

fn update_ambush_warning(
    game_time: Res<GameTime>,
    mut query: Query<&mut Text, With<AmbushWarning>>,
) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };

    if game_time.ambush_warning_timer > 0.0 {
        **text = "⚠ AMBUSH IN PROGRESS ⚠".to_string();
    } else {
        **text = String::new();
    }
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ColonyResource>()
            .add_systems(Startup, setup_ui)
            .add_systems(
                Update,
                (
                    update_day_time,
                    update_resources,
                    update_status_bars,
                    update_ambush_warning,
                ),
            );
    }
}
