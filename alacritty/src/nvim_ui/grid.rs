//! Grid state management for Neovim UI
//!
//! Maintains the grid state and provides conversion to Alacride's rendering format

use std::collections::HashMap;

use crate::display::color::Rgb;
use crate::nvim_ui::protocol::{GridCell as ProtocolGridCell, HighlightAttrs};

/// Grid cell with styling
#[derive(Debug, Clone)]
pub struct GridCell {
    pub character: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub sp: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for GridCell {
    fn default() -> Self {
        Self {
            character: ' ',
            fg: Rgb::new(255, 255, 255),
            bg: Rgb::new(0, 0, 0),
            sp: Rgb::new(255, 0, 0),
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

/// Grid state
pub struct Grid {
    /// Grid dimensions
    width: usize,
    height: usize,
    /// Grid cells (row-major order)
    cells: Vec<GridCell>,
    /// Cursor position
    cursor_row: usize,
    cursor_col: usize,
    /// Default colors
    default_fg: Rgb,
    default_bg: Rgb,
    default_sp: Rgb,
    /// Highlight attribute cache
    hl_attrs: HashMap<u64, HighlightAttrs>,
}

impl Grid {
    /// Create a new grid with the given dimensions
    pub fn new(width: usize, height: usize) -> Self {
        let cells = vec![GridCell::default(); width * height];
        Self {
            width,
            height,
            cells,
            cursor_row: 0,
            cursor_col: 0,
            default_fg: Rgb::new(255, 255, 255),
            default_bg: Rgb::new(0, 0, 0),
            default_sp: Rgb::new(255, 0, 0),
            hl_attrs: HashMap::new(),
        }
    }

    /// Resize the grid
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.cells.resize(width * height, GridCell::default());
    }

    /// Clear the grid
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = GridCell::default();
        }
    }

    /// Set default colors
    pub fn set_default_colors(&mut self, fg: Option<Rgb>, bg: Option<Rgb>, sp: Option<Rgb>) {
        if let Some(fg) = fg {
            self.default_fg = fg;
        }
        if let Some(bg) = bg {
            self.default_bg = bg;
        }
        if let Some(sp) = sp {
            self.default_sp = sp;
        }
    }

    /// Define a highlight attribute
    pub fn define_hl_attr(&mut self, id: u64, attrs: HighlightAttrs) {
        self.hl_attrs.insert(id, attrs);
    }

    /// Update a line on the grid
    pub fn update_line(&mut self, row: usize, col_start: usize, cells: &[ProtocolGridCell]) {
        if row >= self.height {
            return;
        }

        let mut col = col_start;
        for cell_data in cells {
            let repeat = cell_data.repeat as usize;
            let hl_id = cell_data.hl_id;

            // Get highlight attributes
            let hl_attrs = hl_id
                .and_then(|id| self.hl_attrs.get(&id))
                .cloned()
                .unwrap_or_default();

            // Determine colors
            let fg = hl_attrs.foreground.unwrap_or(self.default_fg);
            let bg = hl_attrs.background.unwrap_or(self.default_bg);
            let sp = hl_attrs.special.unwrap_or(self.default_sp);

            // Convert text to characters
            let chars: Vec<char> = cell_data.text.chars().collect();
            let character = chars.first().copied().unwrap_or(' ');

            // Create cell
            let grid_cell = GridCell {
                character,
                fg,
                bg,
                sp,
                bold: hl_attrs.bold,
                italic: hl_attrs.italic,
                underline: hl_attrs.underline || hl_attrs.undercurl,
            };

            // Repeat cell
            for _ in 0..repeat {
                if col < self.width {
                    let idx = row * self.width + col;
                    if idx < self.cells.len() {
                        self.cells[idx] = grid_cell.clone();
                    }
                    col += 1;
                }
            }
        }
    }

    /// Scroll a region of the grid
    pub fn scroll_region(
        &mut self,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        rows: i64,
        _cols: i64,
    ) {
        if rows == 0 {
            return;
        }

        let region_width = right.saturating_sub(left);
        let region_height = bottom.saturating_sub(top);

        if rows > 0 {
            // Scroll down (move content up)
            for row in top..(bottom - rows as usize) {
                let src_row = row + rows as usize;
                if src_row >= self.height {
                    break;
                }
                for col in left..right {
                    if col >= self.width {
                        break;
                    }
                    let src_idx = src_row * self.width + col;
                    let dst_idx = row * self.width + col;
                    if src_idx < self.cells.len() && dst_idx < self.cells.len() {
                        self.cells[dst_idx] = self.cells[src_idx].clone();
                    }
                }
            }
            // Clear exposed lines at bottom
            for row in (bottom - rows as usize)..bottom {
                for col in left..right {
                    if col >= self.width || row >= self.height {
                        break;
                    }
                    let idx = row * self.width + col;
                    if idx < self.cells.len() {
                        self.cells[idx] = GridCell::default();
                    }
                }
            }
        } else {
            // Scroll up (move content down)
            let abs_rows = (-rows) as usize;
            for row in ((top + abs_rows)..bottom).rev() {
                let src_row = row - abs_rows;
                for col in left..right {
                    if col >= self.width {
                        break;
                    }
                    let src_idx = src_row * self.width + col;
                    let dst_idx = row * self.width + col;
                    if src_idx < self.cells.len() && dst_idx < self.cells.len() {
                        self.cells[dst_idx] = self.cells[src_idx].clone();
                    }
                }
            }
            // Clear exposed lines at top
            for row in top..(top + abs_rows) {
                for col in left..right {
                    if col >= self.width || row >= self.height {
                        break;
                    }
                    let idx = row * self.width + col;
                    if idx < self.cells.len() {
                        self.cells[idx] = GridCell::default();
                    }
                }
            }
        }
    }

    /// Set cursor position
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row;
        self.cursor_col = col;
    }

    /// Get cursor position
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    /// Get the top line number from the grid (assumes :set number is enabled)
    /// Returns None if can't parse a line number
    pub fn get_top_line_number(&self) -> Option<u32> {
        if self.height == 0 || self.width < 5 {
            return None;
        }

        // Line numbers are typically in the first ~5 columns
        let line_num_text: String = (0..5.min(self.width))
            .filter_map(|col| {
                let idx = col;  // First row
                if idx < self.cells.len() {
                    let ch = self.cells[idx].character;
                    if ch.is_ascii_digit() || ch == ' ' {
                        Some(ch)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        line_num_text.trim().parse().ok()
    }

    /// Get the bottom visible line number from the grid (assumes :set number is enabled)
    /// Checks the LAST visible row (before buffer rows) - this is rows[n-3] where n is height
    pub fn get_bottom_line_number(&self) -> Option<u32> {
        if self.height < 3 || self.width < 5 {
            return None;
        }

        // Grid has height+2 rows total (includes 2 buffer rows)
        // Last visible row is at index (height - 3)
        // For example: if height=48, visible rows are 0-45, buffer rows are 46-47
        // So check row 45 (which is height-3 = 48-3 = 45)
        let last_visible_row_index = self.height.saturating_sub(3);

        let line_num_text: String = (0..5.min(self.width))
            .filter_map(|col| {
                let idx = last_visible_row_index * self.width + col;
                if idx < self.cells.len() {
                    let ch = self.cells[idx].character;
                    if ch.is_ascii_digit() || ch == ' ' {
                        Some(ch)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        let result = line_num_text.trim().parse().ok();
        eprintln!("🔥 BOTTOM LINE: checking row[{}] (height={}, total rows={}), text='{}' -> {:?}",
                  last_visible_row_index, self.height, self.height, line_num_text, result);
        result
    }

    /// Check if the last row has no line number (we're past the end of content)
    pub fn last_row_is_empty(&self) -> bool {
        if self.height < 1 {
            return false;
        }

        // Check if last row has a line number
        let last_row = self.height - 1;
        let line_num_text: String = (0..5.min(self.width))
            .filter_map(|col| {
                let idx = last_row * self.width + col;
                if idx < self.cells.len() {
                    let ch = self.cells[idx].character;
                    if ch.is_ascii_digit() || ch == ' ' {
                        Some(ch)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        let is_empty = line_num_text.trim().parse::<u32>().is_err();
        if is_empty {
            eprintln!("🔥 BOTTOM CHECK: Last row text=[{}], is_empty={}", line_num_text, is_empty);
        }
        is_empty
    }


    /// Get a cell at the given position
    pub fn get_cell(&self, row: usize, col: usize) -> Option<&GridCell> {
        if row >= self.height || col >= self.width {
            return None;
        }
        let idx = row * self.width + col;
        self.cells.get(idx)
    }

    /// Get all cells (for rendering)
    pub fn cells(&self) -> &[GridCell] {
        &self.cells
    }

    /// Get grid dimensions
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}