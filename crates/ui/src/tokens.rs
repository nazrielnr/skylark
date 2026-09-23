//! Centralized design tokens for Skylark UI.
//!
//! Replaces scattered magic numbers (`px(8.0)`, `wash(0.06)`, hardcoded opacities)
//! with strictly-typed, semantic design constants.

#[cfg(target_os = "windows")]
pub const OVERLAY_CORNER_RADIUS: f32 = 8.0;
#[cfg(not(target_os = "windows"))]
pub const OVERLAY_CORNER_RADIUS: f32 = 12.0;

/// Corner radius for interactive controls (buttons, chips, list items).
pub const CONTROL_CORNER_RADIUS: f32 = 6.0;

/// Shared backdrop blur for floating cards, popovers, and composer pill.
#[cfg(target_os = "windows")]
pub const BACKDROP_BLUR: f32 = 28.0;
#[cfg(not(target_os = "windows"))]
pub const BACKDROP_BLUR: f32 = 16.0;

/// Standard insets and padding tokens.
pub const INSET_SM: f32 = 4.0;
pub const INSET_MD: f32 = 8.0;
pub const INSET_LG: f32 = 12.0;

/// Selection wash alpha values.
pub const SELECTION_WASH_DARK: f32 = 0.11;
pub const SELECTION_WASH_LIGHT: f32 = 0.14;

/// Hairline stroke alpha values.
pub const HAIRLINE_DARK: f32 = 0.12;
pub const HAIRLINE_LIGHT: f32 = 0.08;

/// Acrylic/Mica translucency opacity levels for floating surfaces.
pub const SURFACE_OVERLAY_OPACITY_DARK: f32 = 0.55;
pub const SURFACE_OVERLAY_OPACITY_LIGHT: f32 = 0.70;
