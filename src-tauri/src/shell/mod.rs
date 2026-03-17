use std::sync::Mutex;

use crate::config::{self, AppMode};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;
use tauri::{
    AppHandle, Manager, PhysicalPosition, Position, Runtime, WebviewUrl, WebviewWindowBuilder,
};

const MAIN_WINDOW_LABEL: &str = "main";
const ZEN_PANEL_WINDOW_LABEL: &str = "zen-panel";
const ZEN_PANEL_WIDTH: f64 = 320.0;
const ZEN_PANEL_HEIGHT: f64 = 420.0;
const ZEN_PANEL_OFFSET: i32 = 8;

const MENU_OPEN_ZEN_PANEL: &str = "shell.open_zen_panel";
const MENU_OPEN_DASHBOARD: &str = "shell.open_dashboard";
const MENU_SWITCH_TO_ZEN: &str = "shell.switch_to_zen";
const MENU_SWITCH_TO_FULL: &str = "shell.switch_to_full";
const MENU_QUIT: &str = "shell.quit";

#[derive(Debug)]
pub struct AppShellState {
    mode: Mutex<AppMode>,
}

impl AppShellState {
    pub fn new(mode: AppMode) -> Self {
        Self {
            mode: Mutex::new(mode),
        }
    }

    pub fn mode(&self) -> AppMode {
        self.mode.lock().map(|mode| *mode).unwrap_or(AppMode::Full)
    }

    fn set_mode(&self, mode: AppMode) {
        if let Ok(mut current) = self.mode.lock() {
            *current = mode;
        }
    }
}

pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open_zen_panel = MenuItem::with_id(
        app,
        MENU_OPEN_ZEN_PANEL,
        "Open Zen Panel",
        true,
        None::<&str>,
    )?;
    let open_dashboard = MenuItem::with_id(
        app,
        MENU_OPEN_DASHBOARD,
        "Open Dashboard",
        true,
        None::<&str>,
    )?;
    let switch_to_zen =
        MenuItem::with_id(app, MENU_SWITCH_TO_ZEN, "Switch to Zen", true, None::<&str>)?;
    let switch_to_full = MenuItem::with_id(
        app,
        MENU_SWITCH_TO_FULL,
        "Switch to Full",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let separator_top = PredefinedMenuItem::separator(app)?;
    let separator_bottom = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &open_zen_panel,
            &open_dashboard,
            &separator_top,
            &switch_to_zen,
            &switch_to_full,
            &separator_bottom,
            &quit,
        ],
    )?;

    let mut tray_builder = TrayIconBuilder::with_id("main")
        .tooltip("Echo")
        .show_menu_on_left_click(false)
        .menu(&menu)
        .on_menu_event(|app, event| {
            if let Err(err) = handle_tray_menu_event(app, event) {
                eprintln!("tray menu action failed: {err}");
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let Err(err) = handle_tray_icon_event(tray.app_handle(), event) {
                eprintln!("tray icon action failed: {err}");
            }
        });

    if let Some(icon) = app.default_window_icon() {
        tray_builder = tray_builder.icon(icon.clone());
    }

    tray_builder.build(app)?;
    Ok(())
}

pub fn apply_app_mode<R: Runtime>(
    app: &AppHandle<R>,
    requested_mode: AppMode,
) -> tauri::Result<AppMode> {
    let applied_mode = effective_mode_for_platform(requested_mode, cfg!(target_os = "macos"));
    app.state::<AppShellState>().set_mode(applied_mode);

    #[cfg(target_os = "macos")]
    {
        match applied_mode {
            AppMode::Zen => {
                app.set_activation_policy(ActivationPolicy::Accessory)?;
                if let Some(main_window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                    let _ = main_window.hide();
                }
            }
            AppMode::Full => {
                app.set_activation_policy(ActivationPolicy::Regular)?;
                show_dashboard(app);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = requested_mode;
        show_dashboard(app);
    }

    if applied_mode == AppMode::Full {
        hide_zen_panel(app);
    }

    Ok(applied_mode)
}

fn handle_tray_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) -> tauri::Result<()> {
    match event.id().as_ref() {
        MENU_OPEN_ZEN_PANEL => toggle_zen_panel(app, None)?,
        MENU_OPEN_DASHBOARD => show_dashboard(app),
        MENU_SWITCH_TO_ZEN => switch_mode(app, AppMode::Zen)?,
        MENU_SWITCH_TO_FULL => switch_mode(app, AppMode::Full)?,
        MENU_QUIT => app.exit(0),
        _ => {}
    }
    Ok(())
}

fn handle_tray_icon_event<R: Runtime>(
    app: &AppHandle<R>,
    event: TrayIconEvent,
) -> tauri::Result<()> {
    if let TrayIconEvent::Click {
        button,
        button_state,
        position,
        ..
    } = event
    {
        if button == MouseButton::Left && button_state == MouseButtonState::Up {
            if app.state::<AppShellState>().mode() == AppMode::Zen {
                toggle_zen_panel(app, Some(position))?;
            } else {
                show_dashboard(app);
            }
        }
    }
    Ok(())
}

fn switch_mode<R: Runtime>(app: &AppHandle<R>, mode: AppMode) -> tauri::Result<()> {
    set_and_apply_mode(app, mode)?;
    Ok(())
}

pub fn set_and_apply_mode<R: Runtime>(
    app: &AppHandle<R>,
    requested_mode: AppMode,
) -> tauri::Result<AppMode> {
    let applied_mode = effective_mode_for_platform(requested_mode, cfg!(target_os = "macos"));
    config::set_app_mode(applied_mode).map_err(tauri::Error::from)?;
    apply_app_mode(app, applied_mode)
}

fn show_dashboard<R: Runtime>(app: &AppHandle<R>) {
    if let Some(main_window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = main_window.show();
        let _ = main_window.unminimize();
        let _ = main_window.set_focus();
    }
}

fn hide_zen_panel<R: Runtime>(app: &AppHandle<R>) {
    if let Some(zen_panel) = app.get_webview_window(ZEN_PANEL_WINDOW_LABEL) {
        let _ = zen_panel.hide();
    }
}

fn resolve_zen_anchor_point<R: Runtime>(
    app: &AppHandle<R>,
    anchor_hint: Option<PhysicalPosition<f64>>,
) -> Option<PhysicalPosition<f64>> {
    anchor_hint.or_else(|| app.cursor_position().ok())
}

fn compute_zen_panel_position<R: Runtime>(
    app: &AppHandle<R>,
    anchor: PhysicalPosition<f64>,
) -> PhysicalPosition<i32> {
    let panel_width = ZEN_PANEL_WIDTH.round() as i32;
    let panel_height = ZEN_PANEL_HEIGHT.round() as i32;
    let anchor_x = anchor.x.round() as i32;
    let anchor_y = anchor.y.round() as i32;
    let mut target_x = anchor_x - (panel_width / 2);
    let mut target_y = anchor_y + ZEN_PANEL_OFFSET;

    if let Ok(Some(monitor)) = app.monitor_from_point(anchor.x, anchor.y) {
        let work_area = monitor.work_area();
        let min_x = work_area.position.x;
        let min_y = work_area.position.y;
        let max_x = min_x + (work_area.size.width as i32) - panel_width;
        let max_y = min_y + (work_area.size.height as i32) - panel_height;
        let open_below = anchor_y - min_y < (work_area.size.height as i32 / 2);

        if !open_below {
            target_y = anchor_y - panel_height - ZEN_PANEL_OFFSET;
        }

        target_x = if max_x >= min_x {
            target_x.clamp(min_x, max_x)
        } else {
            min_x
        };
        target_y = if max_y >= min_y {
            target_y.clamp(min_y, max_y)
        } else {
            min_y
        };
    } else {
        target_x = target_x.max(0);
        target_y = target_y.max(0);
    }

    PhysicalPosition::new(target_x, target_y)
}

fn position_zen_panel<R: Runtime>(
    app: &AppHandle<R>,
    panel: &tauri::WebviewWindow<R>,
    anchor_hint: Option<PhysicalPosition<f64>>,
) {
    if let Some(anchor) = resolve_zen_anchor_point(app, anchor_hint) {
        let target = compute_zen_panel_position(app, anchor);
        let _ = panel.set_position(Position::Physical(target));
    }
}

fn toggle_zen_panel<R: Runtime>(
    app: &AppHandle<R>,
    anchor_hint: Option<PhysicalPosition<f64>>,
) -> tauri::Result<()> {
    if let Some(zen_panel) = app.get_webview_window(ZEN_PANEL_WINDOW_LABEL) {
        if zen_panel.is_visible().unwrap_or(false) {
            zen_panel.hide()?;
        } else {
            position_zen_panel(app, &zen_panel, anchor_hint);
            zen_panel.show()?;
            let _ = zen_panel.set_focus();
        }
        return Ok(());
    }

    let panel = WebviewWindowBuilder::new(
        app,
        ZEN_PANEL_WINDOW_LABEL,
        WebviewUrl::App("zen-panel".into()),
    )
    .title("Echo Zen")
    .inner_size(ZEN_PANEL_WIDTH, ZEN_PANEL_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .visible(false)
    .skip_taskbar(true)
    .build()?;

    let panel_for_events = panel.clone();
    panel.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(false) = event {
            let _ = panel_for_events.hide();
        }
    });

    position_zen_panel(app, &panel, anchor_hint);
    panel.show()?;
    let _ = panel.set_focus();
    Ok(())
}

pub(crate) fn effective_mode_for_platform(requested_mode: AppMode, is_macos: bool) -> AppMode {
    if is_macos {
        requested_mode
    } else {
        AppMode::Full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_macos_forces_full_mode() {
        assert_eq!(
            effective_mode_for_platform(AppMode::Zen, false),
            AppMode::Full
        );
        assert_eq!(
            effective_mode_for_platform(AppMode::Full, false),
            AppMode::Full
        );
    }

    #[test]
    fn macos_preserves_requested_mode() {
        assert_eq!(
            effective_mode_for_platform(AppMode::Zen, true),
            AppMode::Zen
        );
        assert_eq!(
            effective_mode_for_platform(AppMode::Full, true),
            AppMode::Full
        );
    }
}
