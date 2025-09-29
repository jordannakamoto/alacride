use std::cmp::{max, min};
use std::collections::VecDeque;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use crossfont::Metrics;

use crate::display::SizeInfo;
use crate::display::content::RenderableCell;
use crate::renderer::smooth_scroll_animator::{EasingFunction, SmoothScrollAnimator};

/// Number of extra lines to render above/below viewport for smooth scrolling
/// Increased to support more aggressive smooth scrolling and prevent pop-in
pub const CHUNK_BUFFER_LINES: usize = 100;

/// Maximum number of chunks to keep in memory
const MAX_CHUNKS: usize = 30;

/// A chunk represents a rendered section of terminal content
#[derive(Debug)]
pub struct RenderChunk {
    /// Starting line of this chunk (in terminal coordinates)
    pub start_line: Line,
    /// Number of lines in this chunk
    pub lines: usize,
    /// The rendered cells for this chunk
    pub cells: Vec<RenderableCell>,
    /// Last time this chunk was accessed (for LRU eviction)
    pub last_accessed: std::time::Instant,
}

impl RenderChunk {
    pub fn new(start_line: Line, lines: usize, cells: Vec<RenderableCell>) -> Self {
        Self { start_line, lines, cells, last_accessed: std::time::Instant::now() }
    }

    pub fn contains_line(&self, line: Line) -> bool {
        line >= self.start_line && line < self.start_line + self.lines as i32
    }

    pub fn end_line(&self) -> Line {
        self.start_line + self.lines as i32 - 1
    }

    pub fn touch(&mut self) {
        self.last_accessed = std::time::Instant::now();
    }
}

/// Manages chunks of rendered terminal content for smooth scrolling
#[derive(Debug)]
pub struct ChunkedRenderer {
    /// Cache of rendered chunks
    chunks: VecDeque<RenderChunk>,
    /// Smooth scroll animator for beautiful animations
    animator: SmoothScrollAnimator,
    /// Size of each cell in pixels
    cell_height: f32,
    /// Maximum lines available in terminal (for bounds checking)
    max_terminal_lines: usize,
    /// Current terminal history size
    terminal_history: usize,
}

impl ChunkedRenderer {
    pub fn new() -> Self {
        let mut animator = SmoothScrollAnimator::new();
        // Configure for beautiful smooth scrolling
        animator.set_easing(EasingFunction::EaseInOutCubic);
        animator.set_momentum(true);
        animator.set_friction(0.94); // Slightly stronger damping to avoid micro-oscillations
        animator.set_sensitivity(0.6); // Less sensitive for buttery smooth scrolling

        Self {
            chunks: VecDeque::new(),
            animator,
            cell_height: 0.0,
            max_terminal_lines: 0,
            terminal_history: 0,
        }
    }

    /// Update the cell height when font changes
    pub fn update_cell_height(&mut self, metrics: &Metrics) {
        self.cell_height = metrics.line_height as f32;
    }

    /// Update terminal bounds information
    pub fn update_terminal_bounds(&mut self, screen_lines: usize, history_size: usize) {
        self.max_terminal_lines = screen_lines;
        self.terminal_history = history_size;
    }

    /// Set the pixel-level viewport offset for smooth scrolling
    pub fn set_viewport_offset(&mut self, offset: f32) {
        self.animator.set_position(offset);
    }

    /// Get the current viewport offset (updates animation)
    pub fn viewport_offset(&mut self) -> f32 {
        self.animator.update()
    }

    /// Get the current viewport offset without updating animation
    pub fn viewport_offset_current(&self) -> f32 {
        self.animator.current_position()
    }

    /// Check if smooth scrolling is currently animating
    pub fn is_animating(&self) -> bool {
        self.animator.is_animating()
    }

    /// Check if we need a redraw for smooth animation
    pub fn needs_redraw(&self) -> bool {
        self.animator.needs_redraw()
    }

    /// Normalize the animator offset by consuming any full-line movement into integer lines.
    /// Returns the number of lines to scroll the terminal by to keep residual pixel offset stable.
    pub fn normalize_and_consume_lines(&mut self) -> i32 {
        if self.cell_height <= 0.0 {
            return 0;
        }
        let offset = self.animator.current_position();
        let lines = (offset / self.cell_height) as i32;
        if lines != 0 {
            self.animator.offset(-(lines as f32) * self.cell_height);
        }
        lines
    }

    /// Calculate which lines need to be rendered for the current viewport with bounds checking
    pub fn calculate_render_range(
        &self,
        size_info: &SizeInfo,
        display_offset: usize,
    ) -> (Line, usize) {
        let viewport_lines = size_info.screen_lines() as i32;

        // IMPORTANT: Only use the terminal display_offset for line selection; the animator
        // only affects fractional pixel translation.
        let line_offset = 0i32;

        // Start rendering from buffer lines above the viewport
        let desired_start_line =
            Line(-(display_offset as i32) - line_offset - CHUNK_BUFFER_LINES as i32);

        // Apply bounds checking - don't go beyond available history
        let max_history_line = -(self.terminal_history as i32);
        let bounded_start_line = Line(max(desired_start_line.0, max_history_line));

        // Calculate total lines to render with bounds checking
        let desired_total_lines = viewport_lines + 2 * CHUNK_BUFFER_LINES as i32;

        // Don't render beyond the bottom of the terminal
        let max_end_line = self.max_terminal_lines as i32;
        let actual_end_line = min(bounded_start_line.0 + desired_total_lines, max_end_line);
        let actual_total_lines = max(0, actual_end_line - bounded_start_line.0) as usize;

        (bounded_start_line, actual_total_lines)
    }

    /// Check if a line exists in the terminal (within history or screen bounds)
    pub fn line_exists(&self, line: usize) -> bool {
        // Convert usize to viewport coordinates
        // In viewport coordinates, 0 is the top visible line
        // Lines above are in history with negative indices (in terminal coordinates)
        line < self.max_terminal_lines + self.terminal_history
    }

    /// Get or create a chunk covering the specified line range
    pub fn get_chunk(&mut self, start_line: Line, _lines: usize) -> Option<&mut RenderChunk> {
        // First check if we have a chunk that covers this range
        for chunk in &mut self.chunks {
            if chunk.contains_line(start_line) {
                chunk.touch();
                return Some(chunk);
            }
        }

        None
    }

    /// Add a new chunk to the cache
    pub fn add_chunk(&mut self, chunk: RenderChunk) {
        // Remove oldest chunks if we exceed maximum
        while self.chunks.len() >= MAX_CHUNKS {
            self.evict_oldest_chunk();
        }

        self.chunks.push_back(chunk);
    }

    /// Evict the oldest (least recently used) chunk
    fn evict_oldest_chunk(&mut self) {
        if let Some(oldest_idx) = self.find_oldest_chunk() {
            self.chunks.remove(oldest_idx);
        }
    }

    /// Find the index of the oldest chunk
    fn find_oldest_chunk(&self) -> Option<usize> {
        self.chunks
            .iter()
            .enumerate()
            .min_by_key(|(_, chunk)| chunk.last_accessed)
            .map(|(idx, _)| idx)
    }

    /// Clear all chunks (called when terminal content changes significantly)
    pub fn clear_chunks(&mut self) {
        self.chunks.clear();
    }

    /// Get cells for rendering with bounds checking and smooth scroll offset.
    /// The caller provides the current viewport_offset so animator.update() is not called twice
    /// per frame with different results.
    pub fn get_renderable_cells_with_offset<I>(
        &mut self,
        cells: I,
        size_info: &SizeInfo,
        display_offset: usize,
        pixel_offset: f32,
        line_offset: usize,
    ) -> Vec<RenderableCell>
    where
        I: Iterator<Item = RenderableCell>,
    {
        let actual_display_offset = display_offset.saturating_sub(line_offset);
        let (start_line, total_lines) =
            self.calculate_render_range(size_info, actual_display_offset);

        // Collect cells and adjust their positions based on scroll offset
        let mut adjusted_cells = Vec::new();

        // Fractional pixel offset for sub-cell positioning provided by caller
        let _pixel_offset = pixel_offset;

        for cell in cells {
            // Check if this cell's line exists in the terminal
            if !self.line_exists(cell.point.line) {
                continue; // Skip cells for non-existent lines
            }

            // Do not adjust line indices; we only apply a fractional pixel translation in the shader
            // (GLSL3) or per-vertex (GLES2).
            let adjusted_cell = cell;

            // Handle negative start lines properly for history buffer rendering
            // Convert cell line to signed int for proper comparison with negative start_line
            let cell_line_signed = adjusted_cell.point.line as i32;
            let end_line_signed = start_line.0 + total_lines as i32;

            // Only include cells that are within our render bounds (including negative history lines)
            if cell_line_signed >= start_line.0 && cell_line_signed < end_line_signed {
                adjusted_cells.push(adjusted_cell);
            }
        }

        adjusted_cells
    }

    /// Update viewport offset based on scroll delta (in lines) with bounds checking
    pub fn update_scroll(&mut self, scroll_delta: f32) {
        let pixel_delta = scroll_delta * self.cell_height;

        // Add the delta to the animator for smooth animation
        self.animator.add_scroll_delta(pixel_delta);
    }

    /// Apply a whole-line scroll to keep residual pixel offset stable when the terminal grid
    /// scrolls by an integer number of lines.
    pub fn consume_scrolled_lines(&mut self, lines: i32) {
        if lines != 0 {
            let delta_pixels = -(lines as f32) * self.cell_height;
            self.animator.offset(delta_pixels);
        }
    }

    /// Animate smooth scrolling (legacy method for compatibility)
    pub fn animate_scroll(&mut self, target_offset: f32, _animation_speed: f32) -> bool {
        // Set target position in the animator
        let pixel_offset = target_offset * self.cell_height;
        self.animator.set_position(pixel_offset);
        self.animator.is_animating()
    }

    /// Stop all scrolling animations
    pub fn stop_animation(&mut self) {
        self.animator.stop();
    }

    /// Get the effective render area considering bounds
    pub fn get_effective_render_area(
        &self,
        size_info: &SizeInfo,
        display_offset: usize,
    ) -> (usize, usize) {
        let (start_line, total_lines) = self.calculate_render_range(size_info, display_offset);

        // Count how many lines actually exist
        let mut existing_lines = 0;
        for i in 0..total_lines {
            let line_coord = start_line.0 + i as i32;
            // Convert to usize for line_exists check, handling negative values
            if line_coord >= 0 {
                let line_usize = line_coord as usize;
                if self.line_exists(line_usize) {
                    existing_lines += 1;
                } else {
                    break; // Stop at first non-existent line
                }
            } else {
                // Negative lines don't exist in our coordinate system
                break;
            }
        }

        (start_line.0.max(0) as usize, existing_lines)
    }
}

impl Default for ChunkedRenderer {
    fn default() -> Self {
        Self::new()
    }
}
