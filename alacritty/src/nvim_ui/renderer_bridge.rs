//! Bridge between Neovim UI events and Alacride's smooth scroll renderer
//!
//! This module translates Neovim's grid_scroll events into smooth scroll
//! animations using Alacride's existing smooth scroll infrastructure.

use log::{debug, info};

use crate::display::SizeInfo;
use crate::nvim_ui::protocol::RedrawEvent;
use crate::renderer::Renderer;

/// Manages the integration between Neovim events and rendering
pub struct NvimRendererBridge {
    /// Whether smooth scrolling is enabled for Neovim
    smooth_scroll_enabled: bool,
    /// Last scroll event for aggregation
    last_scroll_rows: i64,
    /// Active scroll region bounds (top row, bottom row) - the region currently being animated
    active_scroll_region: Option<(i64, i64)>,
}

impl NvimRendererBridge {
    /// Create a new renderer bridge
    pub fn new() -> Self {
        Self {
            smooth_scroll_enabled: true,
            last_scroll_rows: 0,
            active_scroll_region: None,
        }
    }

    /// Process a redraw event and apply smooth scrolling if applicable
    pub fn process_event(
        &mut self,
        event: &RedrawEvent,
        renderer: &mut Renderer,
        size_info: &SizeInfo,
    ) {
        match event {
            RedrawEvent::GridScroll { grid, top, bottom, left, right, rows, cols } => {
                self.handle_scroll(*grid, *top, *bottom, *left, *right, *rows, *cols, renderer, size_info);
            }
            RedrawEvent::Flush => {
                // Reset aggregation on flush
                self.last_scroll_rows = 0;
            }
            _ => {}
        }
    }

    /// Handle a grid_scroll event
    fn handle_scroll(
        &mut self,
        grid: u64,
        top: i64,
        bottom: i64,
        left: i64,
        right: i64,
        rows: i64,
        _cols: i64,
        renderer: &mut Renderer,
        size_info: &SizeInfo,
    ) {
        eprintln!("🔥 NVIM GridScroll: grid={}, top={}, bottom={}, left={}, right={}, rows={}",
                  grid, top, bottom, left, right, rows);

        // Don't interfere with mouse wheel smooth scrolling
        // GridScroll events update the grid content in the background,
        // while mouse wheel controls the visual offset
        // Just track the scroll region
        self.active_scroll_region = Some((top, bottom));
        self.last_scroll_rows = rows;
    }

    /// Enable or disable smooth scrolling
    pub fn set_smooth_scroll(&mut self, enabled: bool) {
        info!("Neovim smooth scroll: {}", if enabled { "enabled" } else { "disabled" });
        self.smooth_scroll_enabled = enabled;
    }

    /// Check if smooth scrolling is enabled
    pub fn is_smooth_scroll_enabled(&self) -> bool {
        self.smooth_scroll_enabled
    }

    /// Get the active scroll region (top row, bottom row)
    /// This is the region currently being animated by smooth scrolling
    pub fn active_scroll_region(&self) -> Option<(i64, i64)> {
        self.active_scroll_region
    }

    /// Clear the active scroll region (called when animation completes or window resizes)
    pub fn clear_scroll_region(&mut self) {
        self.active_scroll_region = None;
    }
}

impl Default for NvimRendererBridge {
    fn default() -> Self {
        Self::new()
    }
}