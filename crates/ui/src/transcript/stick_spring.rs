//! Stick-to-bottom spring physics and transcript scroll calculations.
//!
//! Ported from mugen §1e (following the shape of stackblitz/use-stick-to-bottom):
//! while pinned, a per-frame velocity spring glides the viewport toward the
//! list end with a feed-forward term tracking smoothed target growth.

use gpui::{Bounds, ListOffset, Pixels, Point};

use crate::theme::Theme;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Re-engage the bottom pin when the user returns within this many px of the end.
pub const STICK_THRESHOLD_PX: f32 = 70.0;
/// List overdraw beyond the viewport.
pub const OVERDRAW_PX: f32 = 320.0;
/// Show the scroll-to-bottom button beyond this distance from the end.
pub const SCROLL_BUTTON_THRESHOLD_PX: f32 = 320.0;

/// Bound session-local viewport memory independently of total chat history.
pub const MAX_SAVED_VIEWPORTS: usize = 256;
/// Bound locally-authored queue ids waiting to become transcript prompts.
pub const MAX_PENDING_QUEUED_TURNS: usize = 256;

/// Text-selection edge scrolling runs only during a drag. A 24 ms cadence is
/// smooth enough to track text while avoiding a permanent animation-frame loop
/// on low-end devices.
pub const SELECTION_SCROLL_TICK_MS: u64 = 24;
pub const SELECTION_SCROLL_EDGE_PX: f32 = 36.0;
pub const SELECTION_SCROLL_MAX_STEP_PX: f32 = 24.0;

// ---------------------------------------------------------------------------
// Stick-to-bottom spring (mugen §1e — same constants as its DEFAULT_SPRING)
// ---------------------------------------------------------------------------

/// Retains velocity frame-to-frame (higher = more glide).
pub const SPRING_DAMPING: f32 = 0.7;
/// Pull toward the target (higher = snappier).
pub const SPRING_STIFFNESS: f32 = 0.05;
/// Inertia (higher = slower to start/stop).
pub const SPRING_MASS: f32 = 1.25;
/// Reference frame for the fixed-timestep integration (60fps).
pub const SPRING_FRAME_MS: f32 = 1000.0 / 60.0;
/// Cap on simulated frames per tick — a hitch catches up instead of teleporting.
pub const SPRING_MAX_CATCHUP_FRAMES: f32 = 8.0;
/// EMA rate for the feed-forward target-growth estimate.
pub const SPRING_GROWTH_EMA: f32 = 0.12;
/// While streaming, chase up to this many px above the true bottom (keeps the
/// growing tail visible instead of hugging a moving edge).
pub const SPRING_CHASE_MAX_LEAD: f32 = 32.0;
/// Treat as exactly pinned within this distance of the bottom.
pub const AT_BOTTOM_PX: f32 = 2.0;

/// Retain the spring's state this long after landing, so a streaming pause
/// resumes at cruise. Retaining state does not require drawing idle frames.
pub const SPRING_SETTLE_GRACE_MS: u64 = 500;
/// Teleport when farther than this many viewports from the end; glide the rest.
pub const GLIDE_MAX_VIEWPORTS: f32 = 2.5;

/// A freshly-sent prompt rests this far below the transcript viewport's top.
/// The titlebar overlays the full-height list, so its height is part of the
/// inset; the extra 10px matches the first row's breathing room.
pub const OWN_SEND_TOP_INSET_PX: f32 = Theme::TITLEBAR_HEIGHT + 10.0;

/// Epsilon of extra height under the reservation. The runway ends AT the
/// app's bottom — this is not scroll room (24px of it read as a janky
/// overshoot-and-fight zone, user report) — it exists only to keep the held
/// layout out of gpui's shorter-than-viewport regime, where a bottom-aligned
/// list reports no item bounds (sizing goes blind) and position becomes a
/// function of content height instead of the hold. Two pixels of travel is
/// below perception.
pub const OWN_SEND_SCROLL_SLACK_PX: f32 = 2.0;
/// Per-60fps-frame fraction of the remaining entry glide retained (~90%
/// covered in ~230ms, ease-out).
pub const OWN_SEND_GLIDE_RETAIN: f32 = 0.85;
/// The entry glide snaps to the absolute hold within this error.
pub const OWN_SEND_GLIDE_SNAP_PX: f32 = 1.0;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Once offered, keep the scroll-to-bottom control until close to the end. A single
/// 320px threshold made it disappear halfway through a downward scroll gesture.
pub fn jump_visibility(was_shown: bool, distance: f32) -> bool {
    distance
        > if was_shown {
            AT_BOTTOM_PX
        } else {
            SCROLL_BUTTON_THRESHOLD_PX
        }
}

/// A live stream already resting at the end should keep that end anchored as
/// its measured height grows. This is deliberately narrower than `pinned`:
/// users gliding back toward the bottom keep the normal spring behavior.
pub fn should_anchor_live_stream(pinned: bool, distance_from_bottom: f32, streaming: bool) -> bool {
    pinned && streaming && distance_from_bottom <= AT_BOTTOM_PX
}

/// Whether a user scroll should re-engage the bottom pin: inside the 70px
/// stick band *and* moving toward the bottom. Direction matters — a small
/// wheel-up notch near the bottom stays inside the band, and re-sticking
/// on it would snap the view straight back, making the pin unbreakable.
pub fn should_restick(distance: f32, previous_distance: f32) -> bool {
    distance <= STICK_THRESHOLD_PX && distance < previous_distance
}

/// A bounds-free guard for gliding through rows whose heights are still
/// being measured. The provisional reservation is never a scroll target.
pub fn own_turn_glide_crossed(offset: ListOffset, anchor_ix: usize, inset: f32) -> bool {
    offset.item_ix > anchor_ix
        || (offset.item_ix == anchor_ix && f32::from(offset.offset_in_item) > -inset)
}

/// Signed list scroll step for a pointer near a viewport edge.
///
/// GPUI list offsets increase toward the document bottom. The quadratic ramp
/// keeps entry into the edge zone gentle and reaches full speed at the edge.
pub fn selection_scroll_step(bounds: Bounds<Pixels>, position: Point<Pixels>) -> f32 {
    let height = f32::from(bounds.size.height);
    if height <= 0.0 {
        return 0.0;
    }
    let edge = SELECTION_SCROLL_EDGE_PX.min(height / 3.0);
    if edge <= 0.0 {
        return 0.0;
    }
    let y = f32::from(position.y);
    let top = f32::from(bounds.top());
    let bottom = f32::from(bounds.bottom());
    let scaled = |penetration: f32| {
        let t = (penetration / edge).clamp(0.0, 1.0);
        SELECTION_SCROLL_MAX_STEP_PX * t * t
    };
    if y < top + edge {
        -scaled(top + edge - y)
    } else if y > bottom - edge {
        scaled(y - (bottom - edge))
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// StickSpring
// ---------------------------------------------------------------------------

/// Pure stick-to-bottom spring stepper — the mugen `tick()` integration:
/// velocity relaxes toward `(damping·v + stiffness·diff)/mass` per 60fps
/// sub-frame, position advances by `v + target_vel` where `target_vel` is a
/// feed-forward EMA of target growth px/frame, and the chase point sits up to
/// [`SPRING_CHASE_MAX_LEAD`] px above the true bottom proportional to growth.
#[derive(Debug, Clone, Copy)]
pub struct StickSpring {
    /// Spring velocity, px per 60fps frame.
    pub velocity: f32,
    /// Feed-forward: smoothed target growth, px per 60fps frame.
    pub target_vel: f32,
    /// Target observed at the previous tick (`None` = fresh/parked).
    pub last_target: Option<f32>,
}

impl Default for StickSpring {
    fn default() -> Self {
        Self::new()
    }
}

impl StickSpring {
    pub fn new() -> Self {
        Self {
            velocity: 0.0,
            target_vel: 0.0,
            last_target: None,
        }
    }

    /// Park the spring (drops all state; the next tick starts cold).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Residual motion below mugen's settle thresholds (`v < .05 && targetVel
    /// < .05`)?
    pub fn is_idle(&self) -> bool {
        self.velocity < 0.05 && self.target_vel < 0.05
    }

    pub fn needs_frame(distance: f32) -> bool {
        // The spring is clamped to the target. Residual velocity cannot move
        // a viewport already there; virtual-list height estimates can keep
        // that velocity nonzero indefinitely even after a turn completes.
        distance > 0.5
    }

    pub fn target_vel(&self) -> f32 {
        self.target_vel
    }

    /// Advance one tick. `pos`/`target` are scroll offsets in px (larger =
    /// closer to the bottom); `frames` is elapsed time in 60fps frames
    /// (clamped by the caller to [`SPRING_MAX_CATCHUP_FRAMES`]). Returns the
    /// new position: never overshoots `target`, monotone while approaching,
    /// and snaps exactly once within 0.5px.
    pub fn step(&mut self, mut pos: f32, target: f32, mut frames: f32) -> f32 {
        let grew = self.last_target.map_or(0.0, |last| target - last);
        self.last_target = Some(target);
        if grew < -1.0 {
            // Target shrank (row collapse/removal) — growth estimate is stale.
            self.target_vel = 0.0;
        } else {
            let observed = grew.max(0.0) / frames.max(0.25);
            self.target_vel += SPRING_GROWTH_EMA * (observed - self.target_vel);
        }
        let chase = target - (self.target_vel * 9.0).min(SPRING_CHASE_MAX_LEAD);
        let mut v = self.velocity;
        while frames > 0.0 {
            let h = frames.min(1.0);
            frames -= h;
            let diff = (chase - pos).max(0.0);
            v += h * ((SPRING_DAMPING * v + SPRING_STIFFNESS * diff) / SPRING_MASS - v);
            pos = (pos + (v + self.target_vel) * h).min(target);
        }
        self.velocity = v;
        if target - pos <= 0.5 { target } else { pos }
    }
}
