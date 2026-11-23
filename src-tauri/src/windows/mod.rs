//! Overlay windows: the semicircular rail and the summary card.
//!
//! Every native mutation goes through [`controller`] (audit I05): callbacks,
//! tray actions, IPC commands and timers submit jobs, and the jobs run in
//! batches on the main thread, after the callback that produced them. Each job
//! re-reads the state and re-validates generation and revision right before
//! touching a window (audit I03).

mod controller;
mod hover;
mod native;
mod position;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tauri::{
    App, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::sync::lock;
use crate::usage::{Provider, Service, UsageUpdate};
use controller::{Controller, Job};
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
const POINTER_RESET_EVENT: &str = "rail-pointer-reset";
const USAGE_EVENT: &str = "usage-updated";

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

/// Physical work area and scale of the monitor that holds the rail.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    work: Rect,
    scale: f64,
}

impl Geometry {
    fn layout(&self, side: Side, anchor: i32, summary_height: i32) -> Layout {
        layout(self.work, self.scale, side, anchor, summary_height)
    }
}

struct WindowState {
    placement: RailPlacement,
    hover: Hover,
    /// Physical bounds of the visible summary, for the hover zone.
    summary: Option<Rect>,
    /// Slide offsets of the last frame Windows accepted.
    shifts: [u8; 2],
    /// The geometry the rail was last placed with.
    geometry: Option<Geometry>,
}

type State = Mutex<WindowState>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SummarySelection {
    usage: crate::usage::Usage,
    /// Hover selection order. A new selection always has a higher revision.
    revision: u64,
    /// Data order within a revision (the usage publication sequence).
    sequence: u64,
    side: Side,
}

fn index(provider: Provider) -> usize {
    match provider {
        Provider::Codex => 0,
        Provider::ClaudeCode => 1,
    }
}

fn state(app: &AppHandle) -> std::sync::MutexGuard<'_, WindowState> {
    lock(app.state::<State>().inner())
}

fn controller(app: &AppHandle) -> Arc<Controller> {
    Arc::clone(app.state::<Arc<Controller>>().inner())
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
        shifts: [0; 2],
        geometry: None,
    }));
    let controller = Arc::new(Controller::default());
    app.manage(Arc::clone(&controller));

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

    // Callbacks only queue intents. The work runs after they return.
    let queue = Arc::clone(&controller);
    rail.on_window_event(move |event| match event {
        WindowEvent::ScaleFactorChanged { .. } => queue.submit(Job::Layout),
        // Only the tray hides the rail. If something else minimizes it (a
        // "hide other windows" tool, for example), bring it back without focus.
        WindowEvent::Resized(_) => queue.submit(Job::Restore),
        _ => {}
    });

    let handle = app.handle().clone();
    let dispatcher = Arc::clone(&controller);
    std::thread::Builder::new()
        .name("window-controller".into())
        .spawn(move || controller::run(handle, dispatcher, execute))?;
    controller.watch_topology();

    // Setup runs on the main thread outside any window callback.
    place_rail(app.handle(), side, true)
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

/// The geometry of the monitor that should hold the rail.
fn target_geometry(rail: &WebviewWindow) -> tauri::Result<Geometry> {
    let monitor = rail
        .primary_monitor()?
        .ok_or_else(|| io::Error::other("Primary monitor unavailable"))?;
    let work = monitor.work_area();
    Ok(Geometry {
        work: Rect {
            x: work.position.x,
            y: work.position.y,
            width: work.size.width as i32,
            height: work.size.height as i32,
        },
        scale: monitor.scale_factor(),
    })
}

/// The geometry the rail is placed with, or the target when not placed yet.
fn current_geometry(app: &AppHandle) -> tauri::Result<Geometry> {
    if let Some(geometry) = state(app).geometry {
        return Ok(geometry);
    }
    target_geometry(&window(app, RAIL)?)
}

/// Places the collapsed rail with a fresh geometry snapshot. Main thread.
fn place_rail(app: &AppHandle, side: Side, show: bool) -> tauri::Result<()> {
    let rail = window(app, RAIL)?;
    let geometry = target_geometry(&rail)?;
    let layout = geometry.layout(side, 0, 0);
    native::set_indicator_regions(&rail, layout.rail, layout.indicator_circles([0; 2]))?;
    native::place(&rail, layout.rail, show)?;
    let mut state = state(app);
    state.geometry = Some(geometry);
    state.shifts = [0; 2];
    Ok(())
}

fn publish_placement(app: &AppHandle, side: Side) -> tauri::Result<()> {
    let placement = {
        let mut state = state(app);
        state.placement.side = side;
        state.placement.generation = state.placement.generation.wrapping_add(1);
        state.shifts = [0; 2];
        state.placement
    };
    app.emit_to(RAIL, PLACEMENT_EVENT, placement)
}

/// Expands `provider` (or collapses everything) and selects its summary.
fn activate(app: &AppHandle, provider: Option<Provider>) -> tauri::Result<()> {
    let (selection, generation) = {
        let mut state = state(app);
        state.summary = None;
        (state.hover.activate(provider), state.placement.generation)
    };
    controller(app).watch(provider.is_some());
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
    publish(app, selection, None)
}

fn toggle_now(app: &AppHandle) -> tauri::Result<()> {
    let rail = window(app, RAIL)?;
    let side = state(app).placement.side;
    if native::is_visible(&rail)? {
        activate(app, None)?;
        native::hide(&rail)?;
        publish_placement(app, side)
    } else {
        place_rail(app, side, true)
    }
}

fn restore_if_minimized(app: &AppHandle) -> tauri::Result<()> {
    let rail = window(app, RAIL)?;
    if !native::is_minimized(&rail)? {
        return Ok(());
    }
    let side = state(app).placement.side;
    activate(app, None)?;
    place_rail(app, side, true)?;
    publish_placement(app, side)
}

/// Collapses and re-places the rail on `side` with a new geometry snapshot.
fn relayout(app: &AppHandle, side: Side) -> tauri::Result<()> {
    activate(app, None)?;
    place_rail(app, side, false)?;
    publish_placement(app, side)
}

/// Re-places the rail when the work area, scale or topology changed.
fn check_topology(app: &AppHandle) -> tauri::Result<()> {
    let target = target_geometry(&window(app, RAIL)?)?;
    let (current, side) = {
        let state = state(app);
        (state.geometry, state.placement.side)
    };
    if current != Some(target) {
        relayout(app, side)?;
    }
    Ok(())
}

fn publish(
    app: &AppHandle,
    selection: Option<Selection>,
    update: Option<&UsageUpdate>,
) -> tauri::Result<()> {
    let Some(selection) = selection else {
        return Ok(());
    };
    let fresh;
    let update = match update {
        Some(update) => update,
        None => {
            fresh = app.state::<Service>().update();
            &fresh
        }
    };
    let usage = update
        .snapshot
        .iter()
        .find(|usage| usage.provider == selection.provider)
        .cloned();
    if let Some(usage) = usage {
        let side = state(app).placement.side;
        app.emit_to(
            SUMMARY,
            SUMMARY_EVENT,
            SummarySelection {
                usage,
                revision: selection.revision,
                sequence: update.sequence,
                side,
            },
        )?;
    }
    Ok(())
}

/// Called by the usage workers after every state change. Pushes fresh data
/// without touching window geometry or the running animations.
pub fn usage_changed(app: &AppHandle) {
    let update = app.state::<Service>().update();
    report(app.emit_to(RAIL, USAGE_EVENT, &update));
    let selection = state(app).hover.current();
    report(publish(app, selection, Some(&update)));
}

/// Applies one animation frame, if the placement is still current.
fn motion(app: &AppHandle, generation: u32, shifts: [u8; 2]) -> tauri::Result<bool> {
    let (current, side) = {
        let state = state(app);
        (state.placement.generation, state.placement.side)
    };
    if current != generation {
        return Ok(false);
    }
    let rail = window(app, RAIL)?;
    if !native::is_visible(&rail)? {
        return Ok(false);
    }
    let layout = current_geometry(app)?.layout(side, 0, 0);
    native::set_indicator_regions(&rail, layout.rail, layout.indicator_circles(shifts))?;
    let mut state = state(app);
    if state.placement.generation == generation {
        state.shifts = shifts;
    }
    Ok(true)
}

/// The open intent elapsed. Opens only when the physical cursor is on the
/// indicator as drawn in the last accepted frame (audit I04).
fn intent(app: &AppHandle, epoch: u32) -> tauri::Result<()> {
    let (provider, side, shifts, generation) = {
        let state = state(app);
        (
            state.hover.intent_elapsed(epoch),
            state.placement.side,
            state.shifts,
            state.placement.generation,
        )
    };
    let Some(provider) = provider else {
        return Ok(());
    };
    let rail = window(app, RAIL)?;
    let on_indicator = native::is_visible(&rail)? && {
        let shape = current_geometry(app)?
            .layout(side, 0, 0)
            .indicator_shape(index(provider), shifts);
        native::cursor_position().is_some_and(|(x, y)| shape.contains(x, y))
    };
    if on_indicator {
        return activate(app, Some(provider));
    }
    // A `pointerleave` was lost: forget the pointer, and let the rail report
    // again on the next movement.
    state(app).hover.forget_pointer();
    app.emit_to(RAIL, POINTER_RESET_EVENT, generation)
}

fn check_hover_zone(app: &AppHandle) -> tauri::Result<()> {
    let (provider, side, summary, shifts) = {
        let state = state(app);
        (
            state.hover.active(),
            state.placement.side,
            state.summary,
            state.shifts,
        )
    };
    let Some(provider) = provider else {
        return Ok(());
    };
    let zone =
        current_geometry(app)?
            .layout(side, 0, 0)
            .hover_zone(index(provider), shifts, summary);
    // Without a cursor position, keep the panel open rather than flicker.
    let inside = native::cursor_position().is_none_or(|(x, y)| zone.contains(x, y));
    let close = state(app).hover.tick(inside, Instant::now());
    if close { activate(app, None) } else { Ok(()) }
}

fn show_summary_now(app: &AppHandle, revision: u64, height: i32) -> tauri::Result<()> {
    let (side, provider) = {
        let state = state(app);
        (state.placement.side, state.hover.confirm(revision))
    };
    let Some(provider) = provider else {
        return Ok(());
    };
    let rail = window(app, RAIL)?;
    if !native::is_visible(&rail)? {
        return Ok(());
    }
    let rect = current_geometry(app)?
        .layout(side, anchor(index(provider)), height)
        .summary;
    if rect.width <= 0 || rect.height <= 0 {
        return Ok(());
    }
    // Re-validate right before the mutation: the selection may have changed
    // while this job waited.
    if state(app).hover.confirm(revision).is_none() {
        return Ok(());
    }
    native::place(&window(app, SUMMARY)?, rect, true)?;
    let mut state = state(app);
    if state.hover.confirm(revision).is_some() {
        state.summary = Some(rect);
    }
    Ok(())
}

/// Executes one batch on the main thread.
fn execute(app: &AppHandle, batch: Vec<Job>) {
    for job in batch {
        let result = match job {
            Job::Layout => {
                let side = state(app).placement.side;
                relayout(app, side)
            }
            Job::Restore => restore_if_minimized(app),
            Job::Toggle => toggle_now(app),
            Job::SetSide(side) => relayout(app, side),
            Job::Motion {
                generation,
                shifts,
                reply,
            } => {
                let result = motion(app, generation, shifts).map_err(|error| error.to_string());
                let _ = reply.try_send(result);
                Ok(())
            }
            Job::ShowSummary { revision, height } => show_summary_now(app, revision, height),
            Job::Intent(epoch) => intent(app, epoch),
            Job::HoverTick => check_hover_zone(app),
            Job::TopologyTick => check_topology(app),
        };
        report(result);
    }
}

pub fn toggle(app: &AppHandle) {
    controller(app).submit(Job::Toggle);
}

pub fn set_side(app: &AppHandle, side: Side) {
    controller(app).submit(Job::SetSide(side));
}

#[tauri::command]
pub fn get_rail_placement(app: AppHandle) -> RailPlacement {
    state(&app).placement
}

/// Applies one animation frame: how far each circle has slid inward. Runs as
/// an async command, so it waits for the controller without blocking the main
/// thread that executes the job.
#[tauri::command]
pub async fn set_rail_motion(
    app: AppHandle,
    generation: u32,
    shifts: [u8; 2],
) -> Result<bool, String> {
    if shifts.iter().any(|shift| *shift > SLIDE) {
        return Err("Invalid indicator offset".into());
    }
    if state(&app).placement.generation != generation {
        return Ok(false);
    }
    let (reply, mut answer) = tauri::async_runtime::channel(1);
    controller(&app).submit(Job::Motion {
        generation,
        shifts,
        reply,
    });
    answer.recv().await.unwrap_or(Ok(false))
}

/// The rail reports which indicator is under the pointer. Updates state only:
/// opening is decided when the intent elapses.
#[tauri::command]
pub fn rail_pointer(app: AppHandle, generation: u32, provider: Option<Provider>) {
    let action = {
        let mut state = state(&app);
        if state.placement.generation != generation {
            return;
        }
        state.hover.pointer(provider)
    };
    match action {
        Action::OpenAfter(epoch, delay) => controller(&app).intent(epoch, delay),
        Action::Nothing => {}
    }
}

#[tauri::command]
pub fn summary_ready(app: AppHandle) -> Result<(), String> {
    let selection = state(&app).hover.ready();
    publish(&app, selection, None).map_err(|error| error.to_string())
}

/// The summary rendered `revision` with `height` logical pixels of content.
#[tauri::command]
pub fn show_summary(app: AppHandle, revision: u64, height: i32) {
    if state(&app).hover.confirm(revision).is_some() {
        controller(&app).submit(Job::ShowSummary { revision, height });
    }
}

pub fn report(result: tauri::Result<()>) {
    if let Err(error) = result {
        eprintln!("Aurora Control: {error}");
    }
}
