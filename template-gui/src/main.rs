//! An egui/eframe desktop frontend for the template workspace.
//!
//! A minimal starting point: a window with a text field wired to
//! [`template_core::greeting`]. Its state is persisted across runs through
//! eframe's storage (a RON file in the OS data dir), so it survives a quit or a
//! `bin/main` hot-reload restart. Grow it from here.

use eframe::egui;
use serde::{Deserialize, Serialize};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "template",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// The application state. Runtime-only fields (handles, caches, …) can live here
/// without being persisted; the saved slice is [`Persisted`].
struct App {
    name: String,
}

/// The slice of app state we persist across runs. Kept separate from [`App`] so
/// only the fields we opt into are serialized. Stored through eframe's storage
/// as a RON file in the OS data dir.
#[derive(Serialize, Deserialize)]
struct Persisted {
    name: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            name: "world".to_owned(),
        }
    }
}

impl App {
    /// Build the app, restoring the previous session from eframe's storage if
    /// one was saved.
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default();
        if let Some(saved) = cc
            .storage
            .and_then(|s| eframe::get_value::<Persisted>(s, eframe::APP_KEY))
        {
            app.restore(saved);
        }
        app
    }

    /// Snapshot the persisted slice of state for eframe to serialize.
    fn persisted(&self) -> Persisted {
        Persisted {
            name: self.name.clone(),
        }
    }

    /// Rebuild runtime state from a persisted snapshot.
    fn restore(&mut self, saved: Persisted) {
        self.name = saved.name;
    }
}

impl eframe::App for App {
    /// Persist the view state through eframe's storage.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.persisted());
    }

    /// Autosave frequently: `bin/main`'s hot-reload restart SIGTERMs the window
    /// without a clean shutdown, so a long interval would drop recent edits.
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("template");
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut self.name);
            });
            ui.label(template_core::greeting(&self.name));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::App;

    #[test]
    fn persisted_state_round_trips() {
        let app = App {
            name: "alice".to_owned(),
        };
        let snap = app.persisted();

        let mut restored = App::default();
        restored.restore(snap);
        assert_eq!(restored.name, "alice");
    }
}
