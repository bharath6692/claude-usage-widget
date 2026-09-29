//! The tray context menu.
//!
//! Split in two: a pure model of what should be shown/checked (testable
//! without touching Win32) and a pure command handler that folds a selected
//! item back into [`Settings`]. The thin Win32 rendering (building the actual
//! `HMENU` and calling `TrackPopupMenuEx`) lives in `tray.rs`, where it has
//! nothing left to decide.

use std::path::PathBuf;
use std::time::Duration;

use crate::config::{DisplayMode, Settings, ThemePref, ALLOWED_INTERVALS_SECS};

/// Threshold percentages offered in the Alerts submenu. Independent of
/// whatever subset is currently active in `Settings::thresholds`.
pub const CANDIDATE_THRESHOLDS: [u8; 5] = [50, 70, 80, 90, 95];

// Command IDs doubling as Win32 menu item IDs. Ranges leave room to grow
// without colliding; index-within-range recovers which interval/threshold.
pub const ID_REFRESH_NOW: u16 = 1;
pub const ID_SHOW_DETAILS: u16 = 2;
const ID_INTERVAL_BASE: u16 = 100;
const ID_DISPLAY_DOCKED: u16 = 200;
const ID_DISPLAY_TRAY_ONLY: u16 = 201;
const ID_THEME_AUTO: u16 = 210;
const ID_THEME_DARK: u16 = 211;
const ID_THEME_LIGHT: u16 = 212;
const ID_ALERTS_ENABLED: u16 = 220;
const ID_THRESHOLD_BASE: u16 = 230;
const ID_START_WITH_WINDOWS: u16 = 240;
const ID_LOCK_POSITION: u16 = 241;
const ID_OPEN_SETTINGS_FILE: u16 = 242;
const ID_OPEN_LOG_FILE: u16 = 243;
const ID_CHECK_FOR_UPDATES: u16 = 250;
const ID_ABOUT: u16 = 251;
pub const ID_QUIT: u16 = 260;

#[derive(Debug, Clone, PartialEq)]
pub enum MenuNode {
    Action { id: u16, label: String, enabled: bool },
    Toggle { id: u16, label: String, checked: bool, enabled: bool },
    Submenu { label: String, items: Vec<MenuNode> },
    Separator,
}

/// Everything the tray needs to build its context menu, in display order.
pub fn build(settings: &Settings, has_auth_problem: bool) -> Vec<MenuNode> {
    vec![
        MenuNode::Action {
            id: ID_REFRESH_NOW,
            label: "Refresh now".into(),
            enabled: true,
        },
        MenuNode::Action {
            id: ID_SHOW_DETAILS,
            label: "Usage details...".into(),
            enabled: true,
        },
        MenuNode::Submenu {
            label: "Update frequency".into(),
            items: ALLOWED_INTERVALS_SECS
                .iter()
                .enumerate()
                .map(|(i, secs)| MenuNode::Toggle {
                    id: ID_INTERVAL_BASE + i as u16,
                    label: interval_label(*secs),
                    checked: settings.poll_interval_secs == *secs,
                    enabled: true,
                })
                .collect(),
        },
        MenuNode::Submenu {
            label: "Display".into(),
            items: vec![
                MenuNode::Toggle {
                    id: ID_DISPLAY_DOCKED,
                    label: "Docked to taskbar".into(),
                    checked: settings.display_mode == DisplayMode::Docked,
                    enabled: true,
                },
                MenuNode::Toggle {
                    id: ID_DISPLAY_TRAY_ONLY,
                    label: "Tray only".into(),
                    checked: settings.display_mode == DisplayMode::TrayOnly,
                    enabled: true,
                },
            ],
        },
        MenuNode::Submenu {
            label: "Theme".into(),
            items: vec![
                MenuNode::Toggle {
                    id: ID_THEME_AUTO,
                    label: "Auto".into(),
                    checked: settings.theme == ThemePref::Auto,
                    enabled: true,
                },
                MenuNode::Toggle {
                    id: ID_THEME_DARK,
                    label: "Dark".into(),
                    checked: settings.theme == ThemePref::Dark,
                    enabled: true,
                },
                MenuNode::Toggle {
                    id: ID_THEME_LIGHT,
                    label: "Light".into(),
                    checked: settings.theme == ThemePref::Light,
                    enabled: true,
                },
            ],
        },
        MenuNode::Submenu {
            label: "Alerts".into(),
            items: {
                let mut items = vec![
                    MenuNode::Toggle {
                        id: ID_ALERTS_ENABLED,
                        label: "Enabled".into(),
                        checked: settings.alerts_enabled,
                        enabled: true,
                    },
                    MenuNode::Separator,
                ];
                items.extend(CANDIDATE_THRESHOLDS.iter().enumerate().map(|(i, pct)| {
                    MenuNode::Toggle {
                        id: ID_THRESHOLD_BASE + i as u16,
                        label: format!("{pct}%"),
                        checked: settings.thresholds.contains(pct),
                        // Toggling thresholds when alerts are off is confusing:
                        // grey them out rather than let the state drift unseen.
                        enabled: settings.alerts_enabled,
                    }
                }));
                items
            },
        },
        MenuNode::Submenu {
            label: "Settings".into(),
            items: vec![
                MenuNode::Toggle {
                    id: ID_START_WITH_WINDOWS,
                    label: "Start with Windows".into(),
                    checked: settings.start_with_windows,
                    enabled: true,
                },
                MenuNode::Toggle {
                    id: ID_LOCK_POSITION,
                    label: "Lock position".into(),
                    checked: settings.lock_position,
                    enabled: true,
                },
                MenuNode::Separator,
                MenuNode::Action {
                    id: ID_OPEN_SETTINGS_FILE,
                    label: "Open settings file".into(),
                    enabled: true,
                },
                MenuNode::Action {
                    id: ID_OPEN_LOG_FILE,
                    label: "Open log file".into(),
                    enabled: true,
                },
            ],
        },
        MenuNode::Separator,
        MenuNode::Action {
            id: ID_CHECK_FOR_UPDATES,
            label: "Check for updates…".into(),
            enabled: true,
        },
        MenuNode::Action {
            id: ID_ABOUT,
            label: "About".into(),
            enabled: true,
        },
        MenuNode::Separator,
        MenuNode::Action {
            id: ID_QUIT,
            label: "Quit".into(),
            enabled: true,
        },
    ]
    .into_iter()
    // Surface the auth problem contextually rather than adding a whole new
    // top-level item: "Refresh now" becomes the natural place to retry.
    .map(|node| match node {
        MenuNode::Action { id, label, enabled } if id == ID_REFRESH_NOW && has_auth_problem => {
            MenuNode::Action {
                id,
                label: format!("{label} (sign-in problem)"),
                enabled,
            }
        }
        other => other,
    })
    .collect()
}

fn interval_label(secs: u64) -> String {
    if secs < 3600 {
        format!("{} min", secs / 60)
    } else {
        format!("{} min", secs / 60)
    }
}

/// What the caller must do in response to a selected menu item, beyond
/// whatever `Settings` mutation already happened.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    None,
    RefreshNow,
    ShowDetails,
    IntervalChanged(Duration),
    AutostartChanged(bool),
    OpenPath(PathBuf),
    ShowAbout,
    CheckForUpdates,
    Quit,
}

/// Fold a selected command ID into `settings`, returning any follow-up the
/// caller needs to perform (persistence is the caller's job too).
pub fn handle_selection(id: u16, settings: &mut Settings) -> MenuAction {
    if id == ID_REFRESH_NOW {
        return MenuAction::RefreshNow;
    }
    if id == ID_SHOW_DETAILS {
        return MenuAction::ShowDetails;
    }
    if id == ID_QUIT {
        return MenuAction::Quit;
    }
    if id == ID_DISPLAY_DOCKED {
        settings.display_mode = DisplayMode::Docked;
        return MenuAction::None;
    }
    if id == ID_DISPLAY_TRAY_ONLY {
        settings.display_mode = DisplayMode::TrayOnly;
        return MenuAction::None;
    }
    if id == ID_THEME_AUTO {
        settings.theme = ThemePref::Auto;
        return MenuAction::None;
    }
    if id == ID_THEME_DARK {
        settings.theme = ThemePref::Dark;
        return MenuAction::None;
    }
    if id == ID_THEME_LIGHT {
        settings.theme = ThemePref::Light;
        return MenuAction::None;
    }
    if id == ID_ALERTS_ENABLED {
        settings.alerts_enabled = !settings.alerts_enabled;
        return MenuAction::None;
    }
    if id == ID_START_WITH_WINDOWS {
        settings.start_with_windows = !settings.start_with_windows;
        return MenuAction::AutostartChanged(settings.start_with_windows);
    }
    if id == ID_LOCK_POSITION {
        settings.lock_position = !settings.lock_position;
        return MenuAction::None;
    }
    if id == ID_OPEN_SETTINGS_FILE {
        return MenuAction::OpenPath(crate::config::settings_path());
    }
    if id == ID_OPEN_LOG_FILE {
        return MenuAction::OpenPath(crate::config::log_path());
    }
    if id == ID_CHECK_FOR_UPDATES {
        return MenuAction::CheckForUpdates;
    }
    if id == ID_ABOUT {
        return MenuAction::ShowAbout;
    }
    if (ID_INTERVAL_BASE..ID_INTERVAL_BASE + ALLOWED_INTERVALS_SECS.len() as u16).contains(&id) {
        let secs = ALLOWED_INTERVALS_SECS[(id - ID_INTERVAL_BASE) as usize];
        settings.poll_interval_secs = secs;
        return MenuAction::IntervalChanged(Duration::from_secs(secs));
    }
    if (ID_THRESHOLD_BASE..ID_THRESHOLD_BASE + CANDIDATE_THRESHOLDS.len() as u16).contains(&id) {
        let pct = CANDIDATE_THRESHOLDS[(id - ID_THRESHOLD_BASE) as usize];
        if let Some(pos) = settings.thresholds.iter().position(|t| *t == pct) {
            settings.thresholds.remove(pos);
        } else {
            settings.thresholds.push(pct);
            settings.thresholds.sort_unstable();
        }
        return MenuAction::None;
    }
    MenuAction::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(items: &'a [MenuNode], label: &str) -> &'a MenuNode {
        items
            .iter()
            .find(|n| match n {
                MenuNode::Action { label: l, .. }
                | MenuNode::Toggle { label: l, .. }
                | MenuNode::Submenu { label: l, .. } => l == label,
                MenuNode::Separator => false,
            })
            .unwrap_or_else(|| panic!("no menu item labeled {label}"))
    }

    fn submenu_items<'a>(items: &'a [MenuNode], label: &str) -> &'a [MenuNode] {
        match find(items, label) {
            MenuNode::Submenu { items, .. } => items,
            other => panic!("{label} is not a submenu: {other:?}"),
        }
    }

    #[test]
    fn current_interval_is_the_only_one_checked() {
        let settings = Settings {
            poll_interval_secs: 900,
            ..Default::default()
        };
        let menu = build(&settings, false);
        let items = submenu_items(&menu, "Update frequency");
        let checked: Vec<&str> = items
            .iter()
            .filter_map(|n| match n {
                MenuNode::Toggle { label, checked: true, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(checked, vec!["15 min"]);
    }

    #[test]
    fn display_mode_reflects_settings() {
        let settings = Settings {
            display_mode: DisplayMode::TrayOnly,
            ..Default::default()
        };
        let menu = build(&settings, false);
        let items = submenu_items(&menu, "Display");
        assert!(matches!(
            items[0],
            MenuNode::Toggle { checked: false, .. }
        ));
        assert!(matches!(items[1], MenuNode::Toggle { checked: true, .. }));
    }

    #[test]
    fn threshold_checkboxes_reflect_the_active_set() {
        let settings = Settings {
            thresholds: vec![70, 90],
            ..Default::default()
        };
        let menu = build(&settings, false);
        let items = submenu_items(&menu, "Alerts");
        let checked: Vec<&str> = items
            .iter()
            .filter_map(|n| match n {
                MenuNode::Toggle { label, checked: true, .. } if label.ends_with('%') => {
                    Some(label.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(checked, vec!["70%", "90%"]);
    }

    #[test]
    fn thresholds_are_greyed_out_when_alerts_are_disabled() {
        let settings = Settings {
            alerts_enabled: false,
            ..Default::default()
        };
        let menu = build(&settings, false);
        let items = submenu_items(&menu, "Alerts");
        for item in items {
            if let MenuNode::Toggle { label, enabled, .. } = item {
                if label.ends_with('%') {
                    assert!(!enabled, "{label} should be disabled");
                }
            }
        }
    }

    #[test]
    fn refresh_now_flags_an_auth_problem() {
        let settings = Settings::default();
        let normal = build(&settings, false);
        let flagged = build(&settings, true);
        assert!(matches!(&normal[0], MenuNode::Action { label, .. } if label == "Refresh now"));
        assert!(
            matches!(&flagged[0], MenuNode::Action { label, .. } if label.contains("sign-in problem"))
        );
    }

    #[test]
    fn selecting_display_docked_updates_settings_and_needs_no_followup() {
        let mut settings = Settings {
            display_mode: DisplayMode::TrayOnly,
            ..Default::default()
        };
        let action = handle_selection(ID_DISPLAY_DOCKED, &mut settings);
        assert_eq!(settings.display_mode, DisplayMode::Docked);
        assert_eq!(action, MenuAction::None);
    }

    #[test]
    fn selecting_an_interval_updates_settings_and_signals_the_poller() {
        let mut settings = Settings::default();
        let action = handle_selection(ID_INTERVAL_BASE + 2, &mut settings);
        assert_eq!(settings.poll_interval_secs, 900);
        assert_eq!(action, MenuAction::IntervalChanged(Duration::from_secs(900)));
    }

    #[test]
    fn toggling_a_threshold_adds_and_removes_it() {
        let mut settings = Settings::default(); // starts with [80, 95]
        let id_70 = ID_THRESHOLD_BASE + 1; // CANDIDATE_THRESHOLDS[1] == 70
        handle_selection(id_70, &mut settings);
        assert!(settings.thresholds.contains(&70));

        handle_selection(id_70, &mut settings);
        assert!(!settings.thresholds.contains(&70));
    }

    #[test]
    fn toggling_start_with_windows_signals_autostart_change() {
        let mut settings = Settings::default();
        let action = handle_selection(ID_START_WITH_WINDOWS, &mut settings);
        assert!(settings.start_with_windows);
        assert_eq!(action, MenuAction::AutostartChanged(true));

        let action2 = handle_selection(ID_START_WITH_WINDOWS, &mut settings);
        assert!(!settings.start_with_windows);
        assert_eq!(action2, MenuAction::AutostartChanged(false));
    }

    #[test]
    fn refresh_and_quit_do_not_mutate_settings() {
        let mut settings = Settings::default();
        let before = settings.clone();
        assert_eq!(handle_selection(ID_REFRESH_NOW, &mut settings), MenuAction::RefreshNow);
        assert_eq!(settings, before);
        assert_eq!(handle_selection(ID_QUIT, &mut settings), MenuAction::Quit);
        assert_eq!(settings, before);
    }

    #[test]
    fn unknown_id_is_a_harmless_no_op() {
        let mut settings = Settings::default();
        let before = settings.clone();
        assert_eq!(handle_selection(9999, &mut settings), MenuAction::None);
        assert_eq!(settings, before);
    }
}
