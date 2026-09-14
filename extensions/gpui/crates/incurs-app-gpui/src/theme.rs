//! Colors and metrics for the desktop workbench.
//!
//! The palette follows the window's system appearance so a shipped
//! application matches the desktop it runs on rather than imposing its own
//! look. Both palettes are defined in full; neither is derived from the other.

use gpui::{Hsla, Pixels, WindowAppearance, px, rgb};

/// The resolved palette and metrics for one window appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Window background behind every surface.
    pub background: Hsla,
    /// Raised surface such as the sidebar or a result card.
    pub surface: Hsla,
    /// Surface shown for a hovered, selectable row.
    pub surface_hover: Hsla,
    /// Surface shown for the selected command.
    pub surface_selected: Hsla,
    /// Hairline separator and control border.
    pub border: Hsla,
    /// Primary reading color.
    pub text: Hsla,
    /// Secondary color for help text and labels.
    pub text_muted: Hsla,
    /// Color for text drawn on top of [`Theme::accent`].
    pub text_on_accent: Hsla,
    /// Primary action color.
    pub accent: Hsla,
    /// Primary action color while hovered.
    pub accent_hover: Hsla,
    /// Color for failure states.
    pub danger: Hsla,
    /// Background for a failure banner.
    pub danger_surface: Hsla,
    /// Color for success states.
    pub success: Hsla,
    /// Background for a text control.
    pub field: Hsla,
    /// Selection highlight inside a text control.
    pub selection: Hsla,
}

impl Theme {
    /// Returns the palette matching a window's system appearance.
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    /// The light palette.
    pub fn light() -> Self {
        Self {
            background: rgb(0xf6f6f7).into(),
            surface: rgb(0xffffff).into(),
            surface_hover: rgb(0xececed).into(),
            surface_selected: rgb(0xe3e8f4).into(),
            border: rgb(0xdcdcde).into(),
            text: rgb(0x1c1c1e).into(),
            text_muted: rgb(0x6b6b70).into(),
            text_on_accent: rgb(0xffffff).into(),
            accent: rgb(0x2f6df6).into(),
            accent_hover: rgb(0x2258d8).into(),
            danger: rgb(0xb3261e).into(),
            danger_surface: rgb(0xfdeceb).into(),
            success: rgb(0x1c7c47).into(),
            field: rgb(0xffffff).into(),
            selection: rgb(0xb9d2ff).into(),
        }
    }

    /// The dark palette.
    pub fn dark() -> Self {
        Self {
            background: rgb(0x191a1c).into(),
            surface: rgb(0x212327).into(),
            surface_hover: rgb(0x2b2e33).into(),
            surface_selected: rgb(0x2d3950).into(),
            border: rgb(0x35373c).into(),
            text: rgb(0xececee).into(),
            text_muted: rgb(0x9a9aa2).into(),
            text_on_accent: rgb(0xffffff).into(),
            accent: rgb(0x4d84ff).into(),
            accent_hover: rgb(0x6a99ff).into(),
            danger: rgb(0xf2867c).into(),
            danger_surface: rgb(0x3a2422).into(),
            success: rgb(0x5dd39e).into(),
            field: rgb(0x1a1b1e).into(),
            selection: rgb(0x2f4a7a).into(),
        }
    }
}

impl gpui::Global for Theme {}

/// Width of the command sidebar.
pub const SIDEBAR_WIDTH: Pixels = px(248.0);

/// Height of a single-line text control.
pub const FIELD_HEIGHT: Pixels = px(32.0);

/// Corner radius shared by cards and controls.
pub const RADIUS: Pixels = px(6.0);
