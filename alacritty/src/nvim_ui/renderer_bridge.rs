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
        if !self.smooth_scroll_enabled {
            return;
        }

        eprintln!("🔥 NVIM GridScroll: grid={}, top={}, bottom={}, left={}, right={}, rows={}",
                  grid, top, bottom, left, right, rows);

        // Update the active scroll region - this is the region currently being animated
        self.active_scroll_region = Some((top, bottom));

        // Neovim has already updated the grid content to the NEW position
        // We need to offset it back to the OLD position, then animate to 0
        //
        // If rows=-1: content scrolled up (show it at old position: +1 line = +26px offset)
        // If rows=+1: content scrolled down (show it at old position: -1 line = -26px offset)
        //
        // So the initial offset is OPPOSITE of the scroll direction (no negation)
        let pixel_offset = (rows as f32) * size_info.cell_height();

        eprintln!("🔥 NVIM Setting initial offset: {}px to region ({}, {}) - will animate to 0",
                  pixel_offset, top, bottom);

        // Set the offset directly (bypasses bounds checking)
        renderer.set_nvim_scroll_offset(pixel_offset);

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