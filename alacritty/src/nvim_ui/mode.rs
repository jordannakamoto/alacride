//! Neovim mode manager
//!
//! Manages the Neovim UI state, grid rendering, and event processing

use log::{debug, error, info};

use crate::display::content::RenderableCell;
use crate::display::SizeInfo;
use crate::nvim_ui::{Grid, NvimClient, NvimEvent, NvimRendererBridge, RedrawEvent};
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
        })
    }

    /// Process Neovim events and update grid state
    pub fn process_events(&mut self, renderer: &mut Renderer, size_info: &SizeInfo) {
        let events = self.client.poll_events();

        if !events.is_empty() {
            eprintln!("🔥 NVIM Processing {} events", events.len());
        }

        for event in events {
            match event {
                NvimEvent::Redraw(redraw_events) => {
                    eprintln!("🔥 NVIM Redraw batch with {} events", redraw_events.len());
                    for redraw_event in redraw_events {
                        if matches!(redraw_event, RedrawEvent::GridScroll { .. }) {
                            eprintln!("🔥 NVIM Found GridScroll event!");
                        }
                        self.handle_redraw_event(&redraw_event, renderer, size_info);
                    }
                }
                NvimEvent::Response(response) => {
                    debug!("Received response: {:?}", response);
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

    /// Get renderable cells from the grid
    pub fn get_renderable_cells(&self) -> impl Iterator<Item = RenderableCell> + '_ {
        let (width, height) = self.grid.dimensions();
        let (cursor_row, cursor_col) = self.grid.cursor();

        (0..height).flat_map(move |row| {
            (0..width).filter_map(move |col| {
                self.grid.get_cell(row, col).map(|cell| {
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

                    RenderableCell {
                        point: Point { line: row, column: Column(col) },
                        character: cell.character,
                        extra: None,
                        flags,
                        bg_alpha: 1.0,
                        fg: cell.fg,
                        bg: cell.bg,
                        underline: cell.sp,
                    }
                })
            })
        })
    }

    /// Send input to Neovim
    pub fn send_input(&mut self, input: &str) -> Result<(), String> {
        self.client.input(input)
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
}