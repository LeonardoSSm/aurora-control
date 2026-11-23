mod hover;
mod native;
mod position;

use std::{io, sync::Mutex};

use serde::Serialize;
use tauri::{
    App, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::usage::{Provider, Service, Usage};
use hover::{Hover, Selection};
use position::{Layout, RAIL_HEIGHT, RAIL_WIDTH, Rect, SUMMARY_HEIGHT, SUMMARY_WIDTH, layout};

pub use position::Side;

const RAIL: &str = "rail";
const SUMMARY: &str = "summary";
const SUMMARY_EVENT: &str = "summary-selection";

struct WindowState {
    side: Side,
    hover: Hover,
}

type State = Mutex<WindowState>;

#[derive(Clone, Serialize)]
struct SummarySelection {
    usage: Usage,
    revision: u32,
}

pub fn setup(app: &App) -> tauri::Result<()> {
    app.manage(Mutex::new(WindowState {
        side: Side::Right,
        hover: Hover::default(),
    }));
    let rail = create(
        app,
        RAIL,
        "index.html",
        "Aurora Control",
        RAIL_WIDTH,
        RAIL_HEIGHT,
    )?;
    let summary = create(
        app,
        SUMMARY,
        "summary.html",
        "Aurora Control — Resumo",
        SUMMARY_WIDTH,
        SUMMARY_HEIGHT,
    )?;
    summary.set_ignore_cursor_events(true)?;

    let handle = app.handle().clone();
    rail.on_window_event(move |event| {
        if matches!(event, WindowEvent::ScaleFactorChanged { .. }) {
            let app = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                let side = app.state::<State>().lock().unwrap().side;
                report(set_side(&app, side));
            });
        }
    });

    place_rail(&rail, Side::Right, true)
}

fn create(
    app: &App,
    label: &str,
    page: &'static str,
    title: &str,
    width: i32,
    height: i32,
) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title(title)
        .inner_size(f64::from(width), f64::from(height))
        .visible(false)
        .resizable(false)
        .decorations(false)
        .shadow(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .on_navigation(move |url| {
            url.scheme() == "http"
                && url.host_str() == Some("tauri.localhost")
                && url.port().is_none()
                && (url.path().strip_prefix('/') == Some(page)
                    || (page == "index.html" && url.path() == "/"))
        })
        .build()?;
    native::prevent_activation(&window)?;
    Ok(window)
}

fn window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    app.get_webview_window(label)
        .ok_or_else(|| io::Error::other(format!("Missing window: {label}")).into())
}

fn current_layout(rail: &WebviewWindow, side: Side, anchor: i32) -> tauri::Result<Layout> {
    let monitor = rail
        .primary_monitor()?
        .ok_or_else(|| io::Error::other("Primary monitor unavailable"))?;
    let work = monitor.work_area();
    let area = Rect {
        x: work.position.x,
        y: work.position.y,
        width: work.size.width as i32,
        height: work.size.height as i32,
    };
    Ok(layout(area, monitor.scale_factor(), side, anchor))
}

fn place_rail(rail: &WebviewWindow, side: Side, show: bool) -> tauri::Result<()> {
    let layout = current_layout(rail, side, 0)?;
    native::set_rounded_region(rail, layout.rail, layout.rail_corner_diameter)?;
    native::place(rail, layout.rail, show)
}

fn hide_summary(app: &AppHandle) -> tauri::Result<()> {
    app.state::<State>().lock().unwrap().hover.select(None);
    native::hide(&window(app, SUMMARY)?)
}

pub fn toggle(app: &AppHandle) -> tauri::Result<()> {
    let rail = window(app, RAIL)?;
    if native::is_visible(&rail)? {
        hide_summary(app)?;
        native::hide(&rail)
    } else {
        let side = app.state::<State>().lock().unwrap().side;
        place_rail(&rail, side, true)
    }
}

pub fn set_side(app: &AppHandle, side: Side) -> tauri::Result<()> {
    hide_summary(app)?;
    place_rail(&window(app, RAIL)?, side, false)?;
    app.state::<State>().lock().unwrap().side = side;
    Ok(())
}

fn publish(app: &AppHandle, selection: Option<Selection>) -> tauri::Result<()> {
    if let Some(selection) = selection {
        let service = app.state::<Service>();
        if let Some(usage) = service
            .snapshot()
            .into_iter()
            .find(|usage| usage.provider == selection.provider)
        {
            app.emit_to(
                SUMMARY,
                SUMMARY_EVENT,
                SummarySelection {
                    usage,
                    revision: selection.revision,
                },
            )?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn set_hover(app: AppHandle, provider: Option<Provider>) -> Result<(), String> {
    let result = (|| {
        let rail = window(&app, RAIL)?;
        let summary = window(&app, SUMMARY)?;
        let provider = if native::is_visible(&rail)? {
            provider
        } else {
            None
        };
        let selection = app.state::<State>().lock().unwrap().hover.select(provider);
        native::hide(&summary)?;
        publish(&app, selection)
    })();
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub fn summary_ready(app: AppHandle) -> Result<(), String> {
    let selection = app.state::<State>().lock().unwrap().hover.ready();
    publish(&app, selection).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn show_summary(app: AppHandle, revision: u32) -> Result<(), String> {
    let (side, provider) = {
        let state = app.state::<State>();
        let state = state.lock().unwrap();
        (state.side, state.hover.confirm(revision))
    };
    let Some(provider) = provider else {
        return Ok(());
    };

    let result = (|| {
        let rail = window(&app, RAIL)?;
        if native::is_visible(&rail)? {
            // CSS fixes the first ring at y=32..92 and places the next row 83 px below it.
            let anchor = match provider {
                Provider::Codex => 62,
                Provider::ClaudeCode => 145,
            };
            let rect = current_layout(&rail, side, anchor)?.summary;
            if rect.width > 0 && rect.height > 0 {
                native::place(&window(&app, SUMMARY)?, rect, true)?;
            }
        }
        Ok::<_, tauri::Error>(())
    })();
    result.map_err(|error| error.to_string())
}

pub fn report(result: tauri::Result<()>) {
    if let Err(error) = result {
        eprintln!("Aurora Control: {error}");
    }
}
