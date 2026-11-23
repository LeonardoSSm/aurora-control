#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tray;
mod usage;
mod windows;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(usage::Service::new(usage::DemoAdapter));
            windows::setup(app)?;
            tray::setup(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            usage::get_usage,
            windows::get_rail_placement,
            windows::set_rail_expansion,
            windows::set_hover,
            windows::summary_ready,
            windows::show_summary,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Aurora Control");
}
