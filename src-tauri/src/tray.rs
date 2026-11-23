use tauri::{
    App,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::windows::{self, Side};

pub fn setup(app: &App) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Mostrar / ocultar", true, None::<&str>)?;
    let left = MenuItem::with_id(app, "left", "Mover para a esquerda", true, None::<&str>)?;
    let right = MenuItem::with_id(app, "right", "Mover para a direita", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Encerrar", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &left, &right, &separator, &quit])?;

    let mut tray = TrayIconBuilder::new()
        .tooltip("Aurora Control · Demonstração")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => windows::report(windows::toggle(app)),
            "left" => windows::report(windows::set_side(app, Side::Left)),
            "right" => windows::report(windows::set_side(app, Side::Right)),
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
                windows::report(windows::toggle(tray.app_handle()));
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
