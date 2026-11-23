#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod sync;
mod tray;
mod usage;
mod windows;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let service = usage::Service::from_environment();
            let demo = service.is_demo();
            app.manage(service);
            windows::setup(app)?;
            tray::setup(app, demo)?;
            let handle = app.handle().clone();
            app.state::<usage::Service>()
                .start(Box::new(move || windows::usage_changed(&handle)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            usage::get_usage,
            usage::refresh_usage,
            windows::get_rail_placement,
            windows::set_rail_motion,
            windows::rail_pointer,
            windows::summary_ready,
            windows::show_summary,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Aurora Control");
}
