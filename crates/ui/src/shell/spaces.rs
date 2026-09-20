//! Spaces sidebar: the space-filter dropdown (searchable, with "All projects"),
//! the filtered Sessions list, and the add-space palette (device
//! tabs + filtered folder browser).
//!
//! A space = a synced (device, folder) pair. Spaces stopped being a
//! navigation spine when tabs went device-local: the dropdown only FILTERS
//! the sidebar's session list (never the tab strip) and hosts space
//! management (add via the palette; rename/delete via row context menus).
//! Child module of `shell` so it renders straight off `Shell`'s private state.

use super::*;

pub(crate) mod pins;
pub(crate) use pins::*;
pub(in crate::shell) mod dropdown;
pub(in crate::shell) use dropdown::*;
pub(in crate::shell) mod sessions_view;
pub(in crate::shell) use sessions_view::*;
pub(in crate::shell) mod add_flow;
pub(in crate::shell) use add_flow::*;
pub(in crate::shell) mod dialogs;
pub(in crate::shell) use dialogs::*;

#[cfg(test)]
mod tests;
