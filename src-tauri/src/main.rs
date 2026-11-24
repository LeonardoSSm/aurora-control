#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod autostart;
mod instance;
mod preferences;
mod settings;
mod sync;
mod tray;
mod usage;
mod windows;

use tauri::Manager;

fn main() {
    // One instance per Windows session, claimed before any worker starts. A
    // second launch exits without touching the providers.
    let _instance = match instance::claim(instance::NAME) {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => return,
        Err(error) => {
            // Never block start-up on the guard itself.
            eprintln!("Aurora Control: single-instance check failed: {error}");
            None
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let config =
                preferences::debug_config_dir().or_else(|| app.path().app_config_dir().ok());
            app.manage(preferences::Store::load(config));
            let service = usage::Service::from_environment();
            let demo = service.is_demo();
            app.manage(service);
            app.manage(alerts::Alerts::default());
            windows::setup(app)?;
            tray::setup(app, demo)?;
            let handle = app.handle().clone();
            app.state::<usage::Service>().start(Box::new(move || {
                windows::usage_changed(&handle);
                alerts::evaluate(&handle);
            }));
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
            settings::get_settings,
            settings::set_preferences,
            settings::get_diagnostics,
            alerts::send_test_notification,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Aurora Control");
}

/// IPC permission matrix with the real capabilities (audit M01). The handler
/// answers "ok" to anything that passes the ACL, so a refusal can only come
/// from the capability check.
#[cfg(test)]
mod ipc_tests {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
    use tauri::webview::InvokeRequest;
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    const RAIL: &[&str] = &[
        "get_usage",
        "refresh_usage",
        "get_rail_placement",
        "set_rail_motion",
        "rail_pointer",
    ];
    const SUMMARY: &[&str] = &["summary_ready", "show_summary"];
    const SETTINGS: &[&str] = &[
        "get_settings",
        "set_preferences",
        "get_diagnostics",
        "send_test_notification",
    ];

    fn app() -> tauri::App<MockRuntime> {
        mock_builder()
            .invoke_handler(|invoke| {
                invoke.resolver.resolve("ok");
                true
            })
            .build(tauri::generate_context!(test = true))
            .expect("mock app")
    }

    /// Whether the capability check let `command` through. Argument errors
    /// after the check count as allowed: only the ACL refusal counts as denied.
    fn allowed(window: &tauri::WebviewWindow<MockRuntime>, command: &str) -> bool {
        let request = InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        };
        match get_ipc_response(window, request) {
            Ok(_) => true,
            Err(error) => !error.to_string().contains("not allowed"),
        }
    }

    #[test]
    fn each_window_reaches_only_its_own_commands() {
        let app = app();
        let all: Vec<&str> = [RAIL, SUMMARY, SETTINGS].concat();
        for (label, own) in [("rail", RAIL), ("summary", SUMMARY), ("settings", SETTINGS)] {
            let window =
                WebviewWindowBuilder::new(&app, label, WebviewUrl::App("index.html".into()))
                    .build()
                    .unwrap();
            for command in &all {
                assert_eq!(
                    allowed(&window, command),
                    own.contains(command),
                    "{label} → {command}"
                );
            }
            // Only the overlays listen to events; the settings window does not.
            assert_eq!(
                allowed(&window, "plugin:event|listen"),
                label != "settings",
                "{label} → listen"
            );
            // Core plugins beyond event listening are not granted.
            for command in [
                "plugin:window|set_position",
                "plugin:webview|create_webview_window",
                "plugin:event|emit",
                "plugin:path|resolve_directory",
                "plugin:notification|notify",
                "plugin:notification|request_permission",
            ] {
                assert!(!allowed(&window, command), "{label} → {command}");
            }
        }
    }

    #[test]
    fn an_unknown_window_gets_nothing() {
        let app = app();
        let window =
            WebviewWindowBuilder::new(&app, "intruder", WebviewUrl::App("index.html".into()))
                .build()
                .unwrap();
        for command in [RAIL, SUMMARY, SETTINGS].concat() {
            assert!(!allowed(&window, command), "intruder → {command}");
        }
    }
}
