mod hover;
mod native;
mod position;

use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{
    App, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::usage::{Provider, Service, Usage};
use hover::{Action, Hover, Selection};
use position::{
    Layout, RAIL_HEIGHT, RAIL_WIDTH, Rect, SLIDE, SUMMARY_MIN_HEIGHT, SUMMARY_WIDTH, anchor, layout,
};

pub use position::Side;

const RAIL: &str = "rail";
const SUMMARY: &str = "summary";
const SUMMARY_EVENT: &str = "summary-selection";
const PLACEMENT_EVENT: &str = "rail-placement";
const ACTIVE_EVENT: &str = "rail-active";
const USAGE_EVENT: &str = "usage-updated";
const WATCH_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Serialize)]
pub struct RailPlacement {
    side: Side,
    generation: u32,
}

#[derive(Clone, Copy, Serialize)]
struct RailActive {
    generation: u32,
    provider: Option<Provider>,
}

struct WindowState {
    placement: RailPlacement,
    hover: Hover,
    /// Physical bounds of the visible summary, for the hover zone.
    summary: Option<Rect>,
}

type State = Mutex<WindowState>;

/// Wakes the cursor watchdog while an indicator is active.
#[derive(Default)]
struct Watch {
    active: Mutex<bool>,
    wake: Condvar,
}

#[derive(Clone, Serialize)]
struct SummarySelection {
    usage: Usage,
    revision: u32,
    side: Side,
}

fn index(provider: Provider) -> usize {
    match provider {
        Provider::Codex => 0,
        Provider::ClaudeCode => 1,
    }
}

pub fn setup(app: &App) -> tauri::Result<()> {
    // `AURORA_SIDE=left` starts on the left edge. The tray changes it at runtime.
    let side = match std::env::var("AURORA_SIDE").as_deref() {
        Ok("left") => Side::Left,
        _ => Side::Right,
    };
    app.manage(Mutex::new(WindowState {
        placement: RailPlacement {
            side,
            generation: 0,
        },
        hover: Hover::default(),
        summary: None,
    }));
    let watch = Arc::new(Watch::default());
    app.manage(Arc::clone(&watch));

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
        SUMMARY_MIN_HEIGHT,
    )?;
    // The card is informative only. Clicks pass through to the app beneath it;
    // the hover zone keeps it open while the cursor rests on it.
    summary.set_ignore_cursor_events(true)?;

    let handle = app.handle().clone();
    rail.on_window_event(move |event| match event {
        WindowEvent::ScaleFactorChanged { .. } => {
            let app = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                let side = app.state::<State>().lock().unwrap().placement.side;
                report(set_side(&app, side));
            });
        }
        // Only the tray hides the rail. If something else minimizes it (a
        // "hide other windows" tool, for example), bring it back without focus.
        WindowEvent::Resized(_) => {
            let app = handle.clone();
            let _ = handle.run_on_main_thread(move || report(restore_if_minimized(&app)));
        }
        _ => {}
    });

    let handle = app.handle().clone();
    std::thread::Builder::new()
        .name("hover-watchdog".into())
        .spawn(move || watchdog(&handle, &watch))?;

    place_rail(&rail, side, true)
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
    native::make_overlay(&window)?;
    Ok(window)
}

fn window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    app.get_webview_window(label)
        .ok_or_else(|| io::Error::other(format!("Missing window: {label}")).into())
}

fn current_layout(
    rail: &WebviewWindow,
    side: Side,
    anchor: i32,
    summary_height: i32,
) -> tauri::Result<Layout> {
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
    Ok(layout(
        area,
        monitor.scale_factor(),
        side,
        anchor,
        summary_height,
    ))
}

fn place_rail(rail: &WebviewWindow, side: Side, show: bool) -> tauri::Result<()> {
    let layout = current_layout(rail, side, 0, 0)?;
    native::set_indicator_regions(rail, layout.rail, layout.indicator_circles([0; 2]))?;
    native::place(rail, layout.rail, show)
}

fn publish_placement(app: &AppHandle, side: Side) -> tauri::Result<()> {
    let placement = {
        let state = app.state::<State>();
        let mut state = state.lock().unwrap();
        state.placement.side = side;
        state.placement.generation = state.placement.generation.wrapping_add(1);
        state.placement
    };
    app.emit_to(RAIL, PLACEMENT_EVENT, placement)
}

/// Expands `provider` (or collapses everything) and selects its summary.
fn activate(app: &AppHandle, provider: Option<Provider>) -> tauri::Result<()> {
    let (selection, generation) = {
        let state = app.state::<State>();
        let mut state = state.lock().unwrap();
        state.summary = None;
        (state.hover.activate(provider), state.placement.generation)
    };
    {
        let watch = app.state::<Arc<Watch>>();
        *watch.active.lock().unwrap() = provider.is_some();
        watch.wake.notify_one();
    }
    // The previous card hides until the new one confirms its render.
    native::hide(&window(app, SUMMARY)?)?;
    app.emit_to(
        RAIL,
        ACTIVE_EVENT,
        RailActive {
            generation,
            provider,
        },
    )?;
    publish(app, selection)
}

pub fn toggle(app: &AppHandle) -> tauri::Result<()> {
    let rail = window(app, RAIL)?;
    let side = app.state::<State>().lock().unwrap().placement.side;
    if native::is_visible(&rail)? {
        activate(app, None)?;
        native::hide(&rail)?;
        publish_placement(app, side)
    } else {
        place_rail(&rail, side, true)
    }
}

fn restore_if_minimized(app: &AppHandle) -> tauri::Result<()> {
    let rail = window(app, RAIL)?;
    if !native::is_minimized(&rail)? {
        return Ok(());
    }
    let side = app.state::<State>().lock().unwrap().placement.side;
    activate(app, None)?;
    place_rail(&rail, side, true)?;
    publish_placement(app, side)
}

pub fn set_side(app: &AppHandle, side: Side) -> tauri::Result<()> {
    activate(app, None)?;
    place_rail(&window(app, RAIL)?, side, false)?;
    publish_placement(app, side)
}

fn publish(app: &AppHandle, selection: Option<Selection>) -> tauri::Result<()> {
    let Some(selection) = selection else {
        return Ok(());
    };
    let usage = app
        .state::<Service>()
        .snapshot()
        .into_iter()
        .find(|usage| usage.provider == selection.provider);
    if let Some(usage) = usage {
        let side = app.state::<State>().lock().unwrap().placement.side;
        app.emit_to(
            SUMMARY,
            SUMMARY_EVENT,
            SummarySelection {
                usage,
                revision: selection.revision,
                side,
            },
        )?;
    }
    Ok(())
}

/// Called by the usage workers after every state change. Pushes fresh data
/// without touching window geometry or the running animations.
pub fn usage_changed(app: &AppHandle) {
    let snapshot = app.state::<Service>().snapshot();
    report(app.emit_to(RAIL, USAGE_EVENT, snapshot));
    let selection = app.state::<State>().lock().unwrap().hover.current();
    report(publish(app, selection));
}

/// Polls the cursor while an indicator is active. All window work runs on the
/// main thread; this thread only paces it.
fn watchdog(app: &AppHandle, watch: &Watch) {
    loop {
        {
            let active = watch.active.lock().unwrap();
            drop(watch.wake.wait_while(active, |active| !*active).unwrap());
        }
        std::thread::sleep(WATCH_INTERVAL);
        let handle = app.clone();
        if app
            .run_on_main_thread(move || report(check_hover_zone(&handle)))
            .is_err()
        {
            return;
        }
    }
}

fn check_hover_zone(app: &AppHandle) -> tauri::Result<()> {
    let (provider, side, summary) = {
        let state = app.state::<State>();
        let state = state.lock().unwrap();
        (state.hover.active(), state.placement.side, state.summary)
    };
    let Some(provider) = provider else {
        return Ok(());
    };
    let rail = window(app, RAIL)?;
    let zone = current_layout(&rail, side, 0, 0)?.hover_zone(index(provider), summary);
    // Without a cursor position, keep the panel open rather than flicker.
    let inside = native::cursor_position().is_none_or(|(x, y)| zone.contains(x, y));
    let close = app
        .state::<State>()
        .lock()
        .unwrap()
        .hover
        .tick(inside, Instant::now());
    if close { activate(app, None) } else { Ok(()) }
}

#[tauri::command]
pub fn get_rail_placement(app: AppHandle) -> RailPlacement {
    app.state::<State>().lock().unwrap().placement
}

/// Applies one animation frame: how far each circle has slid inward.
#[tauri::command]
pub fn set_rail_motion(app: AppHandle, generation: u32, shifts: [u8; 2]) -> Result<bool, String> {
    let placement = get_rail_placement(app.clone());
    if placement.generation != generation {
        return Ok(false);
    }
    if shifts.iter().any(|shift| *shift > SLIDE) {
        return Err("Invalid indicator offset".into());
    }

    let result = (|| {
        let rail = window(&app, RAIL)?;
        if !native::is_visible(&rail)? {
            return Ok(false);
        }
        let layout = current_layout(&rail, placement.side, 0, 0)?;
        native::set_indicator_regions(&rail, layout.rail, layout.indicator_circles(shifts))?;
        Ok::<_, tauri::Error>(true)
    })();
    result.map_err(|error| error.to_string())
}

/// The rail reports which indicator is under the pointer.
#[tauri::command]
pub fn rail_pointer(
    app: AppHandle,
    generation: u32,
    provider: Option<Provider>,
) -> Result<(), String> {
    if get_rail_placement(app.clone()).generation != generation {
        return Ok(());
    }
    let result = (|| {
        let provider = if native::is_visible(&window(&app, RAIL)?)? {
            provider
        } else {
            None
        };
        let action = app.state::<State>().lock().unwrap().hover.pointer(provider);
        match action {
            Action::Nothing => Ok(()),
            Action::OpenAfter(epoch, delay) => {
                let app = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        let provider = handle
                            .state::<State>()
                            .lock()
                            .unwrap()
                            .hover
                            .intent_elapsed(epoch);
                        if let Some(provider) = provider {
                            report(activate(&handle, Some(provider)));
                        }
                    });
                });
                Ok(())
            }
        }
    })();
    result.map_err(|error: tauri::Error| error.to_string())
}

#[tauri::command]
pub fn summary_ready(app: AppHandle) -> Result<(), String> {
    let selection = app.state::<State>().lock().unwrap().hover.ready();
    publish(&app, selection).map_err(|error| error.to_string())
}

/// The summary rendered `revision` with `height` logical pixels of content.
#[tauri::command]
pub fn show_summary(app: AppHandle, revision: u32, height: i32) -> Result<(), String> {
    let (side, provider) = {
        let state = app.state::<State>();
        let state = state.lock().unwrap();
        (state.placement.side, state.hover.confirm(revision))
    };
    let Some(provider) = provider else {
        return Ok(());
    };

    let result = (|| {
        let rail = window(&app, RAIL)?;
        if native::is_visible(&rail)? {
            let rect = current_layout(&rail, side, anchor(index(provider)), height)?.summary;
            if rect.width > 0 && rect.height > 0 {
                native::place(&window(&app, SUMMARY)?, rect, true)?;
                let state = app.state::<State>();
                let mut state = state.lock().unwrap();
                if state.hover.confirm(revision).is_some() {
                    state.summary = Some(rect);
                }
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
