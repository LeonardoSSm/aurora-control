//! Overlay windows (the semicircular rail and the summary card) and the
//! settings window.
//!
//! Every native mutation goes through [`controller`] (audit I05): callbacks,
//! tray actions, IPC commands and timers submit jobs, and the jobs run in
//! batches on the main thread, after the callback that produced them. Each job
//! re-reads the state and re-validates generation and revision right before
//! touching a window (audit I03).
//!
//! Geometry is one snapshot (monitor work area, scale and visible indicators)
//! used for the HWND, the region, the hover zone and the summary. It is taken
//! again when the DPI, the work area, the topology or the preferences change
//! (audit I07).

mod controller;
mod hover;
pub mod monitors;
mod native;
mod position;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tauri::{
    App, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::preferences::{self, Store};
use crate::sync::lock;
use crate::usage::{Provider, Service, UsageUpdate};
use controller::{Controller, Job};
use hover::{Action, Hover, Selection};
use position::{
    Layout, MAX_INDICATORS, RAIL_WIDTH, Rect, SLIDE, SUMMARY_MIN_HEIGHT, SUMMARY_WIDTH, Shifts,
    anchor, capacity, layout_with, rail_height,
};

const COLLAPSED: Shifts = [0; MAX_INDICATORS];

pub use position::Side;

const RAIL: &str = "rail";
const SUMMARY: &str = "summary";
const SETTINGS: &str = "settings";
const SUMMARY_EVENT: &str = "summary-selection";
const PLACEMENT_EVENT: &str = "rail-placement";
const ACTIVE_EVENT: &str = "rail-active";
const POINTER_RESET_EVENT: &str = "rail-pointer-reset";
const USAGE_EVENT: &str = "usage-updated";

#[derive(Clone, Serialize)]
pub struct RailPlacement {
    side: Side,
    generation: u32,
    /// Visible indicators, from the top.
    providers: Vec<Provider>,
}

#[derive(Clone, Copy, Serialize)]
struct RailActive {
    generation: u32,
    provider: Option<Provider>,
}

/// Physical work area and scale of the monitor that holds the rail, and the
/// number of visible indicators.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    work: Rect,
    scale: f64,
    slots: usize,
}

impl Geometry {
    fn layout(&self, side: Side, anchor: i32, summary_height: i32) -> Layout {
        layout_with(
            self.work,
            self.scale,
            side,
            self.slots,
            anchor,
            summary_height,
        )
    }
}

/// Where the rail is, for the settings window.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub monitor: Option<String>,
    /// The preferred monitor is disconnected: the primary is used instead.
    pub fallback: bool,
    /// Indicators drawn. Fewer than the enabled providers only when the work
    /// area cannot fit them all (see `position::capacity`).
    pub shown: usize,
}

struct WindowState {
    placement: RailPlacement,
    hover: Hover,
    /// Physical bounds of the visible summary, for the hover zone.
    summary: Option<Rect>,
    /// Slide offsets of the last frame Windows accepted.
    shifts: Shifts,
    /// The geometry the rail was last placed with.
    geometry: Option<Geometry>,
    monitor: Placement,
    /// Providers the user enabled, in catalog order. `placement.providers`
    /// is the part of it that fits the work area (see `position::capacity`).
    enabled: Vec<Provider>,
}

impl WindowState {
    fn slot(&self, provider: Provider) -> Option<usize> {
        self.placement
            .providers
            .iter()
            .position(|visible| *visible == provider)
    }
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

fn state(app: &AppHandle) -> std::sync::MutexGuard<'_, WindowState> {
    lock(app.state::<State>().inner())
}

fn controller(app: &AppHandle) -> Arc<Controller> {
    Arc::clone(app.state::<Arc<Controller>>().inner())
}

pub fn setup(app: &App) -> tauri::Result<()> {
    let stored = app.state::<Store>().get();
    // Environment > preferences > defaults.
    let (side, _) = preferences::effective_side(&stored);
    let providers = stored.providers.visible();
    let service = app.state::<Service>();
    service.set_visible(&providers);
    service.select_metrics(&stored.providers.metrics());
    app.manage(Mutex::new(WindowState {
        placement: RailPlacement {
            side,
            generation: 0,
            providers: providers.clone(),
        },
        hover: Hover::default(),
        summary: None,
        shifts: COLLAPSED,
        geometry: None,
        monitor: Placement::default(),
        enabled: providers,
    }));
    let controller = Arc::new(Controller::default());
    app.manage(Arc::clone(&controller));

    let rail = create(
        app,
        RAIL,
        "index.html",
        "Aurora Control",
        RAIL_WIDTH,
        rail_height(MAX_INDICATORS),
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
    // Debug builds only: open the settings window at start, for validation
    // scripts that cannot drive the tray menu.
    if cfg!(debug_assertions)
        && std::env::var_os("AURORA_OPEN_SETTINGS").is_some_and(|value| value == "1")
    {
        controller.submit(Job::OpenSettings);
    }

    // Setup runs on the main thread outside any window callback.
    place_rail(app.handle(), side, stored.rail_visible)
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
        .on_navigation(move |url| same_page(url, page))
        .build()?;
    native::make_overlay(&window)?;
    Ok(window)
}

/// Navigation stays on the window's own bundled page.
fn same_page(url: &tauri::Url, page: &str) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("tauri.localhost")
        && url.port().is_none()
        && (url.path().strip_prefix('/') == Some(page)
            || (page == "index.html" && url.path() == "/"))
}

fn window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    app.get_webview_window(label)
        .ok_or_else(|| io::Error::other(format!("Missing window: {label}")).into())
}

/// The geometry the rail should use now: the preferred monitor when it is
/// connected, otherwise the primary, with its own work area and DPI.
fn target_geometry(app: &AppHandle) -> tauri::Result<(Geometry, Placement)> {
    let preferred = app.state::<Store>().get().monitor.map(|monitor| monitor.id);
    let monitors = monitors::enumerate();
    let (monitor, fallback) = monitors::choose(&monitors, preferred.as_deref())
        .ok_or_else(|| io::Error::other("No monitor available"))?;
    let enabled = state(app).enabled.len();
    let slots = enabled.min(capacity(monitor.work.height, monitor.scale));
    Ok((
        Geometry {
            work: monitor.work,
            scale: monitor.scale,
            slots,
        },
        Placement {
            monitor: Some(monitor.id.clone()),
            fallback,
            shown: slots,
        },
    ))
}

/// The geometry the rail is placed with, or the target when not placed yet.
fn current_geometry(app: &AppHandle) -> tauri::Result<Geometry> {
    if let Some(geometry) = state(app).geometry {
        return Ok(geometry);
    }
    Ok(target_geometry(app)?.0)
}

/// Where the rail is placed, for the settings window.
pub fn placement(app: &AppHandle) -> Placement {
    state(app).monitor.clone()
}

/// Places the collapsed rail with a fresh geometry snapshot. Main thread.
/// With no visible provider the rail is hidden and has an empty region: the
/// tray and the settings window stay available.
fn place_rail(app: &AppHandle, side: Side, show: bool) -> tauri::Result<()> {
    let rail = window(app, RAIL)?;
    let (geometry, monitor) = target_geometry(app)?;
    {
        // The indicators drawn are the enabled ones that fit, in order.
        let mut state = state(app);
        let shown: Vec<Provider> = state.enabled.iter().take(geometry.slots).copied().collect();
        state.placement.providers = shown;
    }
    if geometry.slots == 0 {
        native::set_indicator_regions(&rail, Rect::default(), &[])?;
        native::hide(&rail)?;
    } else {
        let layout = geometry.layout(side, 0, 0);
        native::set_indicator_regions(&rail, layout.rail, &layout.visible_circles(COLLAPSED))?;
        native::place(&rail, layout.rail, show)?;
    }
    let mut state = state(app);
    state.geometry = Some(geometry);
    state.monitor = monitor;
    state.shifts = COLLAPSED;
    Ok(())
}

fn publish_placement(app: &AppHandle, side: Side) -> tauri::Result<()> {
    let placement = {
        let mut state = state(app);
        state.placement.side = side;
        state.placement.generation = state.placement.generation.wrapping_add(1);
        state.shifts = COLLAPSED;
        state.placement.clone()
    };
    app.emit_to(RAIL, PLACEMENT_EVENT, placement)
}

/// Remembers whether the rail is shown, so a restart keeps the choice.
fn remember_rail_visible(app: &AppHandle, visible: bool) {
    let store = app.state::<Store>();
    let current = store.get();
    if current.rail_visible == visible {
        return;
    }
    let saved = store.set(preferences::Preferences {
        rail_visible: visible,
        ..current
    });
    if let Err(error) = saved {
        eprintln!("Aurora Control: could not save the rail visibility: {error}");
    }
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
    let (side, nothing_enabled) = {
        let state = state(app);
        (state.placement.side, state.enabled.is_empty())
    };
    if native::is_visible(&rail)? {
        activate(app, None)?;
        native::hide(&rail)?;
        remember_rail_visible(app, false);
        publish_placement(app, side)
    } else if nothing_enabled {
        // Nothing to show: the settings window is where providers are enabled.
        open_settings(app)
    } else {
        remember_rail_visible(app, true);
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

/// Re-places the rail when the work area, scale or topology changed: the
/// taskbar moved, a monitor was attached, removed or made primary.
fn check_topology(app: &AppHandle) -> tauri::Result<()> {
    let (target, _) = target_geometry(app)?;
    let (current, side) = {
        let state = state(app);
        (state.geometry, state.placement.side)
    };
    if current != Some(target) {
        relayout(app, side)?;
    }
    Ok(())
}

/// Applies saved preferences: side, monitor, visible providers, their ring
/// metrics and the rail visibility. Enabling, disabling or reordering
/// providers collapses the rail first and re-places it with a new
/// generation, so a hover in progress can never target a slot that moved.
fn apply_preferences(app: &AppHandle) -> tauri::Result<()> {
    let stored = app.state::<Store>().get();
    let (side, _) = preferences::effective_side(&stored);
    let providers = stored.providers.visible();
    let service = app.state::<Service>();
    service.set_visible(&providers);
    service.select_metrics(&stored.providers.metrics());
    state(app).enabled = providers;
    activate(app, None)?;
    place_rail(app, side, stored.rail_visible)?;
    if !stored.rail_visible {
        native::hide(&window(app, RAIL)?)?;
    }
    publish_placement(app, side)?;
    // A new ring metric changes what the snapshot shows without any new
    // reading: publish it now instead of at the next refresh.
    usage_changed(app);
    Ok(())
}

/// Opens the settings window, or brings it forward.
fn open_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        window.unminimize()?;
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("Aurora Control — Configurações")
        .inner_size(560.0, 720.0)
        .min_inner_size(480.0, 560.0)
        .resizable(true)
        .center()
        .theme(Some(tauri::Theme::Dark))
        .background_color(tauri::window::Color(9, 11, 16, 255))
        .on_navigation(|url| same_page(url, "settings.html"))
        .build()?;
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
fn motion(app: &AppHandle, generation: u32, shifts: Shifts) -> tauri::Result<bool> {
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
    native::set_indicator_regions(&rail, layout.rail, &layout.visible_circles(shifts))?;
    let mut state = state(app);
    if state.placement.generation == generation {
        state.shifts = shifts;
    }
    Ok(true)
}

/// The open intent elapsed. Opens only when the physical cursor is on the
/// indicator as drawn in the last accepted frame (audit I04).
fn intent(app: &AppHandle, epoch: u32) -> tauri::Result<()> {
    let (provider, slot, side, shifts, generation) = {
        let state = state(app);
        let provider = state.hover.intent_elapsed(epoch);
        (
            provider,
            provider.and_then(|provider| state.slot(provider)),
            state.placement.side,
            state.shifts,
            state.placement.generation,
        )
    };
    let (Some(provider), Some(slot)) = (provider, slot) else {
        return Ok(());
    };
    let rail = window(app, RAIL)?;
    let on_indicator = native::is_visible(&rail)? && {
        let shape = current_geometry(app)?
            .layout(side, 0, 0)
            .indicator_shape(slot, shifts);
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
    let (slot, side, summary, shifts) = {
        let state = state(app);
        (
            state
                .hover
                .active()
                .and_then(|provider| state.slot(provider)),
            state.placement.side,
            state.summary,
            state.shifts,
        )
    };
    let Some(slot) = slot else {
        if state(app).hover.active().is_some() {
            // The active provider was hidden meanwhile.
            return activate(app, None);
        }
        return Ok(());
    };
    let zone = current_geometry(app)?
        .layout(side, 0, 0)
        .hover_zone(slot, shifts, summary);
    // Without a cursor position, keep the panel open rather than flicker.
    let inside = native::cursor_position().is_none_or(|(x, y)| zone.contains(x, y));
    let close = state(app).hover.tick(inside, Instant::now());
    if close { activate(app, None) } else { Ok(()) }
}

fn show_summary_now(app: &AppHandle, revision: u64, height: i32) -> tauri::Result<()> {
    let (side, slot) = {
        let state = state(app);
        (
            state.placement.side,
            state
                .hover
                .confirm(revision)
                .and_then(|provider| state.slot(provider)),
        )
    };
    let Some(slot) = slot else {
        return Ok(());
    };
    let rail = window(app, RAIL)?;
    if !native::is_visible(&rail)? {
        return Ok(());
    }
    let rect = current_geometry(app)?
        .layout(side, anchor(slot), height)
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
            Job::Preferences => apply_preferences(app),
            Job::OpenSettings => open_settings(app),
        };
        report(result);
    }
}

pub fn toggle(app: &AppHandle) {
    controller(app).submit(Job::Toggle);
}

/// Moves the rail and remembers the side.
pub fn set_side(app: &AppHandle, side: Side) {
    let store = app.state::<Store>();
    let saved = store.set(preferences::Preferences {
        side,
        ..store.get()
    });
    if let Err(error) = saved {
        eprintln!("Aurora Control: could not save the side: {error}");
    }
    controller(app).submit(Job::SetSide(side));
}

/// Re-applies the saved preferences.
pub fn preferences_changed(app: &AppHandle) {
    controller(app).submit(Job::Preferences);
}

pub fn show_settings(app: &AppHandle) {
    controller(app).submit(Job::OpenSettings);
}

#[tauri::command]
pub fn get_rail_placement(app: AppHandle) -> RailPlacement {
    state(&app).placement.clone()
}

/// Applies one animation frame: how far each circle has slid inward. Runs as
/// an async command, so it waits for the controller without blocking the main
/// thread that executes the job.
#[tauri::command]
pub async fn set_rail_motion(
    app: AppHandle,
    generation: u32,
    shifts: Vec<u8>,
) -> Result<bool, String> {
    if shifts.len() > MAX_INDICATORS || shifts.iter().any(|shift| *shift > SLIDE) {
        return Err("Invalid indicator offset".into());
    }
    // One offset per visible slot; the remaining slots stay collapsed.
    let mut padded = COLLAPSED;
    padded[..shifts.len()].copy_from_slice(&shifts);
    let shifts = padded;
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
        // A hidden provider cannot be hovered.
        let provider = provider.filter(|provider| state.slot(*provider).is_some());
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
