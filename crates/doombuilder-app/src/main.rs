// ABOUTME: Binary entry point. Delegates to the iced application in doombuilder-gui.
// ABOUTME: Kept intentionally tiny so the GUI crate stays the source of truth.

fn main() -> iced::Result {
    doombuilder_gui::run()
}
