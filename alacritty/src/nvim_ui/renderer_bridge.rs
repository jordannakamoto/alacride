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
    /// Current cursor row position (for detecting scroll boundaries)
    cursor_row: u64,
    /// Previous cursor row (to detect if scroll actually happened)
    prev_cursor_row: u64,
    /// Whether we received a grid_scroll event in this frame
    received_grid_scroll: bool,
    /// Whether we're currently at the bottom boundary
    at_bottom_boundary: bool,
    /// Last seen top line number (for detecting when scroll is stuck)
    last_top_line: Option<u32>,
    /// Number of consecutive scroll attempts that didn't move top line
    stuck_scroll_count: u32,
}

impl NvimRendererBridge {
    /// Create a new renderer bridge
    pub fn new() -> Self {
        Self {
            smooth_scroll_enabled: true,
            last_scroll_rows: 0,
            active_scroll_region: None,
            cursor_row: 0,
            prev_cursor_row: 0,
            received_grid_scroll: false,
            at_bottom_boundary: false,
            last_top_line: None,
            stuck_scroll_count: 0,
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
                self.received_grid_scroll = true;
                self.handle_scroll(*grid, *top, *bottom, *left, *right, *rows, *cols, renderer, size_info);
            }
            RedrawEvent::GridCursorGoto { row, .. } => {
                self.prev_cursor_row = self.cursor_row;
                self.cursor_row = *row;
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
        nvim_debug!("🔥 NVIM GridScroll: grid={}, top={}, bottom={}, left={}, right={}, rows={}",
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

    /// Get current cursor row
    pub fn cursor_row(&self) -> u64 {
        self.cursor_row
    }

    /// Check if we're likely at a scroll boundary (top or bottom of file)
    /// by seeing if the cursor didn't move after a scroll attempt
    pub fn at_scroll_boundary(&self) -> bool {
        // If cursor is at row 0 or 1, likely at top of file
        // The cursor position doesn't change much when hitting boundaries
        self.cursor_row <= 1
    }

    /// Check if we received a GridScroll event this frame
    pub fn did_grid_scroll(&self) -> bool {
        self.received_grid_scroll
    }

    /// Reset the GridScroll flag (call after processing a frame)
    pub fn reset_grid_scroll_flag(&mut self) {
        self.received_grid_scroll = false;
    }

    /// Set the bottom boundary flag
    pub fn set_at_bottom_boundary(&mut self, at_bottom: bool) {
        self.at_bottom_boundary = at_bottom;
    }

    /// Check if we're at the bottom boundary
    pub fn is_at_bottom_boundary(&self) -> bool {
        self.at_bottom_boundary
    }

    /// Get last top line
    pub fn get_last_top_line(&self) -> Option<u32> {
        self.last_top_line
    }

    /// Set last top line
    pub fn set_last_top_line(&mut self, line: Option<u32>) {
        self.last_top_line = line;
    }
}

impl Default for NvimRendererBridge {
    fn default() -> Self {
        Self::new()
    }
}