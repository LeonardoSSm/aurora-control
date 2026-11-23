fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_usage",
            "get_rail_placement",
            "set_rail_expansion",
            "set_hover",
            "summary_ready",
            "show_summary",
        ]),
    ))
    .expect("failed to generate Tauri configuration and permissions");
}
