fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_usage",
            "refresh_usage",
            "get_rail_placement",
            "set_rail_motion",
            "rail_pointer",
            "summary_ready",
            "show_summary",
            "get_settings",
            "set_preferences",
            "get_diagnostics",
            "send_test_notification",
        ]),
    ))
    .expect("failed to generate Tauri configuration and permissions");
}
