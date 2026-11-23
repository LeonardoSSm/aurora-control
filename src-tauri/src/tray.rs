use tauri::{
    App, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::usage::Service;
use crate::windows::{self, Side};

pub fn setup(app: &App, demo: bool) -> tauri::Result<()> {
    let refresh = MenuItem::with_id(app, "refresh", "Atualizar agora", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Mostrar / ocultar", true, None::<&str>)?;
    let left = MenuItem::with_id(app, "left", "Mover para a esquerda", true, None::<&str>)?;
    let right = MenuItem::with_id(app, "right", "Mover para a direita", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Configurações…", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Encerrar", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &refresh, &toggle, &left, &right, &settings, &separator, &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::new()
        .tooltip(if demo {
            "Aurora Control · Demonstração"
        } else {
            "Aurora Control"
        })
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "refresh" => app.state::<Service>().refresh(None),
            "toggle" => windows::toggle(app),
            "left" => windows::set_side(app, Side::Left),
            "right" => windows::set_side(app, Side::Right),
            "settings" => windows::show_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                windows::toggle(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
