mod events;
mod map;
mod pawn;
mod ui;

use bevy::prelude::*;

use events::EventsPlugin;
use map::MapPlugin;
use pawn::PawnPlugin;
use ui::UiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins((
            MapPlugin,
            PawnPlugin,
            UiPlugin,
            EventsPlugin,
        ))
        .run();
}