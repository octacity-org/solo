#![deny(clippy::all)]

mod alerts;
mod application;
mod automation;
mod application_commands;
mod custom_elements;
mod desktop;
mod diff;
mod dock;
mod element_tree;
mod file_dialogs;
#[cfg(target_os = "macos")]
mod context_menu;
#[cfg(all(not(target_os = "macos"), ui_thread))]
mod context_menu_gpui;
#[cfg(target_os = "macos")]
mod macos_shutdown;
#[cfg(target_os = "macos")]
mod menu_bar;
mod markdown;
mod motion;
mod native_view;
mod open_urls;
#[cfg(ui_thread)]
mod prompt;
mod renderer;
mod retained_tree;
mod screens;
mod style;
mod system_appearance;
mod syntax;
mod text;
mod theme;

#[cfg(all(feature = "test-support", target_os = "macos"))]
mod test_renderer;

pub use element_tree::*;
pub use renderer::*;
pub use style::*;
