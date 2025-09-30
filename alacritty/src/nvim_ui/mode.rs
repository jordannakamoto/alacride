//! Neovim mode manager
//!
//! Manages the Neovim UI state, grid rendering, and event processing

use log::{debug, error, info};

use crate::display::content::RenderableCell;
use crate::display::color::Rgb;
use crate::display::SizeInfo;
use crate::nvim_ui::{Grid, NvimClient, NvimEvent, NvimRendererBridge, RedrawEvent};
use crate::nvim_ui::grid::GridCell;
use crate::renderer::Renderer;

use alacritty_terminal::index::{Point, Column, Line};
use alacritty_terminal::term::cell::Flags;

/// Neovim mode state
pub struct NvimMode {
    /// Neovim RPC client
    client: NvimClient,
    /// Grid state
    grid: Grid,
    /// Renderer bridge for smooth scrolling
    renderer_bridge: NvimRendererBridge,
    /// Whether the mode is active
    active: bool,
    /// Last line in buffer (from line('$')) - used for bottom boundary detection
    buffer_last_line: Option<u32>,
}

impl NvimMode {
    /// Create a new Neovim mode
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        info!("Initializing Neovim mode");

        let client = NvimClient::spawn(width, height)?;
        let grid = Grid::new(width as usize, height as usize);
        let renderer_bridge = NvimRendererBridge::new();

        Ok(Self {
            client,
            grid,
            renderer_bridge,
            active: true,
            buffer_last_line: None,
        })
    }

    /// Process Neovim events and update grid state
    pub fn process_events(&mut self, renderer: &mut Renderer, size_info: &SizeInfo) {
        let events = self.client.poll_events();

        if !events.is_empty() {
            nvim_debug!("🔥 NVIM Processing {} events", events.len());
        }

        for event in events {
            match event {
                NvimEvent::Redraw(redraw_events) => {
                    nvim_debug!("🔥 NVIM Redraw batch with {} events", redraw_events.len());
                    for redraw_event in redraw_events {
                        if matches!(redraw_event, RedrawEvent::GridScroll { .. }) {
                            nvim_debug!("🔥 NVIM Found GridScroll event!");
                        }
                        self.handle_redraw_event(&redraw_event, renderer, size_info);
                    }
                }
                NvimEvent::Response(response) => {
                    debug!("Received response: {:?}", response);
                    // Check if this is a response to our line('$') query
                    if let Some(result) = &response.result {
                        if let Some(line_num) = result.as_u64() {
                            self.buffer_last_line = Some(line_num as u32);
                            nvim_debug!("🔥 NVIM Buffer last line: {}", line_num);
                        }
                    }
                }
                NvimEvent::Request(request) => {
                    debug!("Received request: {:?}", request);
                }
            }
        }
    }

    /// Handle a single redraw event
    fn handle_redraw_event(
        &mut self,
        event: &RedrawEvent,
        renderer: &mut Renderer,
        size_info: &SizeInfo,
    ) {
        match event {
            RedrawEvent::GridLine { grid, row, col_start, cells } => {
                if *grid == 1 {
                    self.grid.update_line(*row as usize, *col_start as usize, cells);
                }
            }
            RedrawEvent::GridScroll { grid, top, bottom, left, right, rows, cols } => {
                if *grid == 1 {
                    self.grid.scroll_region(
                        *top as usize,
                        *bottom as usize,
                        *left as usize,
                        *right as usize,
                        *rows,
                        *cols,
                    );
                }
                // Forward to renderer bridge for smooth scrolling
                self.renderer_bridge.process_event(event, renderer, size_info);
            }
            RedrawEvent::GridResize { grid, width, height } => {
                if *grid == 1 {
                    self.grid.resize(*width as usize, *height as usize);
                }
            }
            RedrawEvent::GridClear { grid } => {
                if *grid == 1 {
                    self.grid.clear();
                }
            }
            RedrawEvent::GridCursorGoto { grid, row, col } => {
                if *grid == 1 {
                    self.grid.set_cursor(*row as usize, *col as usize);
                }
                // Forward to renderer bridge for cursor tracking
                self.renderer_bridge.process_event(event, renderer, size_info);
            }
            RedrawEvent::DefaultColorsSet { fg, bg, sp } => {
                self.grid.set_default_colors(*fg, *bg, *sp);
            }
            RedrawEvent::HlAttrDefine { id, attrs } => {
                self.grid.define_hl_attr(*id, attrs.clone());
            }
            RedrawEvent::Flush => {
                self.renderer_bridge.process_event(event, renderer, size_info);
            }
            _ => {
                // Ignore other events for now
            }
        }
    }

    /// Get cursor position
    pub fn get_cursor(&self) -> (usize, usize) {
        self.grid.cursor()
    }

    /// Get renderable cells from the grid
    pub fn get_renderable_cells(&self) -> Vec<RenderableCell> {
        let (width, height) = self.grid.dimensions();
        let (cursor_row, cursor_col) = self.grid.cursor();

        // Pre-scan to find selection ranges on each line
        let selection_blue = Rgb::new(70, 130, 255);
        let default_bg = Rgb::new(30, 30, 46); // Approximate default bg

        let mut line_selections: Vec<Option<(usize, usize)>> = vec![None; height];

        for row in 0..height {
            let mut first_selected = None;
            let mut last_selected = None;

            for col in 0..width {
                if let Some(cell) = self.grid.get_cell(row, col) {
                    // Check if this cell has a selection background (bright blue or non-default bg)
                    if cell.bg == selection_blue || (cell.bg != default_bg && cell.bg != Rgb::new(0, 0, 0)) {
                        if first_selected.is_none() {
                            first_selected = Some(col);
                        }
                        last_selected = Some(col);
                    }
                }
            }

            if let (Some(first), Some(last)) = (first_selected, last_selected) {
                line_selections[row] = Some((first, last));
            }
        }

        // Generate cells with filled selection ranges
        let mut cells = Vec::new();

        for row in 0..height {
            for col in 0..width {
                if let Some(cell) = self.grid.get_cell(row, col) {
                    let mut flags = Flags::empty();

                    if cell.bold {
                        flags |= Flags::BOLD;
                    }
                    if cell.italic {
                        flags |= Flags::ITALIC;
                    }
                    if cell.underline {
                        flags |= Flags::UNDERLINE;
                    }

                    // Check if this cell is within a selection range
                    let bg = if let Some((first, last)) = line_selections[row] {
                        if col >= first && col <= last {
                            selection_blue
                        } else {
                            cell.bg
                        }
                    } else {
                        cell.bg
                    };

                    cells.push(RenderableCell {
                        point: Point { line: row, column: Column(col) },
                        character: cell.character,
                        extra: None,
                        flags,
                        bg_alpha: 1.0,
                        fg: cell.fg,
                        bg,
                        underline: cell.sp,
                    });
                }
            }
        }

        cells
    }

    /// Send input to Neovim
    pub fn send_input(&mut self, input: &str) -> Result<(), String> {
        self.client.input(input)
    }

    /// Execute a Vim command directly (doesn't trigger keymaps)
    pub fn exec_command(&mut self, command: &str) -> Result<(), String> {
        self.client.exec_command(command)
    }

    /// Resize the Neovim UI
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.grid.resize(width as usize, height as usize);
        self.client.resize(width, height)
    }

    /// Check if the mode is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Deactivate the mode
    pub fn deactivate(&mut self) {
        info!("Deactivating Neovim mode");
        self.active = false;
    }

    /// Get the active scroll region (top row, bottom row)
    pub fn active_scroll_region(&self) -> Option<(i64, i64)> {
        self.renderer_bridge.active_scroll_region()
    }

    /// Clear the scroll region (called on resize)
    pub fn clear_scroll_region(&mut self) {
        self.renderer_bridge.clear_scroll_region();
    }

    /// Check if we're at a scroll boundary (top or bottom of file)
    pub fn at_scroll_boundary(&self) -> bool {
        self.renderer_bridge.at_scroll_boundary()
    }

    /// Check if Neovim sent a GridScroll event (indicates scroll actually happened)
    pub fn did_grid_scroll(&self) -> bool {
        self.renderer_bridge.did_grid_scroll()
    }

    /// Reset the GridScroll flag after checking
    pub fn reset_grid_scroll_flag(&mut self) {
        self.renderer_bridge.reset_grid_scroll_flag();
    }

    /// Get the top line number from grid (for boundary detection)
    pub fn get_top_line_number(&self) -> Option<u32> {
        self.grid.get_top_line_number()
    }

    /// Get the bottom line number from grid (for boundary detection)
    pub fn get_bottom_line_number(&self) -> Option<u32> {
        self.grid.get_bottom_line_number()
    }

    /// Set the bottom boundary flag
    pub fn set_at_bottom_boundary(&mut self, at_bottom: bool) {
        self.renderer_bridge.set_at_bottom_boundary(at_bottom);
    }

    /// Check if we're at the bottom boundary
    pub fn is_at_bottom_boundary(&self) -> bool {
        self.renderer_bridge.is_at_bottom_boundary()
    }

    /// Check if the last row is empty (no line number)
    pub fn last_row_is_empty(&self) -> bool {
        self.grid.last_row_is_empty()
    }

    /// Get last top line
    pub fn get_last_top_line(&self) -> Option<u32> {
        self.renderer_bridge.get_last_top_line()
    }

    /// Set last top line
    pub fn set_last_top_line(&mut self, line: Option<u32>) {
        self.renderer_bridge.set_last_top_line(line);
    }

    /// Query the buffer's last line using Neovim API
    /// This updates the internal buffer_last_line cache
    pub fn query_buffer_last_line(&mut self) -> Result<(), String> {
        // Query line('$') to get the last line in buffer
        self.client.eval_expr("line('$')")?;
        Ok(())
    }

    /// Check if we're at the bottom - stop when buffer's last line is at the top of viewport
    pub fn is_at_buffer_bottom(&self) -> bool {
        let visible_top = self.grid.get_top_line_number();
        let buffer_last = self.buffer_last_line;

        // Check if buffer's last line is at or above the top of the screen
        let result = if let (Some(buffer_last), Some(visible_top)) = (buffer_last, visible_top) {
            // We're at bottom if the top visible row shows the buffer's last line (or beyond)
            let at_bottom = visible_top >= buffer_last;
            nvim_debug!("🔥 BOTTOM CHECK: visible_top={}, buffer_last={}, at_bottom={}",
                      visible_top, buffer_last, at_bottom);
            at_bottom
        } else if visible_top.is_none() {
            // Can't parse line number from top row
            nvim_debug!("🔥 BOTTOM CHECK: Top row is blank (visible_top=None) - AT BOTTOM");
            true
        } else {
            // Don't have buffer info yet
            nvim_debug!("🔥 BOTTOM CHECK: No buffer info yet - visible_top={:?}, buffer_last={:?}",
                      visible_top, buffer_last);
            false
        };

        result
    }
}