fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_usage",
            "set_hover",
            "summary_ready",
            "show_summary",
        ]),
    ))
    .expect("failed to generate Tauri configuration and permissions");
}
