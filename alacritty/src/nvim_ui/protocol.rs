//! Neovim RPC Protocol Types
//!
//! Defines the message types and event parsing for Neovim's UI protocol

use log::{debug, warn};
use rmpv::Value;

use crate::display::color::Rgb;

/// Events received from Neovim
#[derive(Debug, Clone)]
pub enum NvimEvent {
    /// UI redraw event with batched updates
    Redraw(Vec<RedrawEvent>),
    /// Response to a request
    Response(NvimResponse),
    /// Request from Neovim (rare)
    Request(NvimRequest),
}

/// Response from Neovim
#[derive(Debug, Clone)]
pub struct NvimResponse {
    pub id: u64,
    pub error: Value,
    pub result: Option<Value>,
}

/// Request from Neovim to client
#[derive(Debug, Clone)]
pub struct NvimRequest {
    pub id: u64,
    pub method: String,
    pub params: Value,
}

/// Individual redraw events
#[derive(Debug, Clone)]
pub enum RedrawEvent {
    /// Grid line update
    GridLine {
        grid: u64,
        row: u64,
        col_start: u64,
        cells: Vec<GridCell>,
    },
    /// Grid scroll
    GridScroll {
        grid: u64,
        top: i64,
        bottom: i64,
        left: i64,
        right: i64,
        rows: i64,
        cols: i64,
    },
    /// Grid resize
    GridResize {
        grid: u64,
        width: u64,
        height: u64,
    },
    /// Clear grid
    GridClear {
        grid: u64,
    },
    /// Cursor goto
    GridCursorGoto {
        grid: u64,
        row: u64,
        col: u64,
    },
    /// Set default colors
    DefaultColorsSet {
        fg: Option<Rgb>,
        bg: Option<Rgb>,
        sp: Option<Rgb>,
    },
    /// Highlight attribute definition
    HlAttrDefine {
        id: u64,
        attrs: HighlightAttrs,
    },
    /// Mode info set
    ModeInfoSet {
        cursor_style_enabled: bool,
        mode_info: Vec<ModeInfo>,
    },
    /// Mode change
    ModeChange {
        mode_name: String,
        mode_idx: u64,
    },
    /// Flush (end of redraw batch)
    Flush,
    /// Other/unknown events
    Other(String),
}

/// Grid cell data
#[derive(Debug, Clone)]
pub struct GridCell {
    pub text: String,
    pub hl_id: Option<u64>,
    pub repeat: u64,
}

/// Highlight attributes
#[derive(Debug, Clone, Default)]
pub struct HighlightAttrs {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub special: Option<Rgb>,
    pub reverse: bool,
    pub italic: bool,
    pub bold: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub blend: Option<u8>,
}

/// Mode info
#[derive(Debug, Clone)]
pub struct ModeInfo {
    pub cursor_shape: Option<String>,
    pub cell_percentage: Option<u64>,
    pub blinkwait: Option<u64>,
    pub blinkon: Option<u64>,
    pub blinkoff: Option<u64>,
}

/// Parse a notification message
pub fn parse_notification(method: &str, params: Value) -> Result<NvimEvent, String> {
    match method {
        "redraw" => {
            let events = parse_redraw_events(params)?;
            Ok(NvimEvent::Redraw(events))
        }
        other => {
            debug!("Unhandled notification: {}", other);
            Ok(NvimEvent::Redraw(vec![RedrawEvent::Other(other.to_string())]))
        }
    }
}

/// Parse redraw event batch
fn parse_redraw_events(params: Value) -> Result<Vec<RedrawEvent>, String> {
    let mut events = Vec::new();
    let array = params.as_array().ok_or("Expected array")?;

    for event_batch in array {
        let batch_array = event_batch.as_array().ok_or("Expected event batch array")?;
        if batch_array.is_empty() {
            continue;
        }

        let event_name = batch_array[0]
            .as_str()
            .ok_or("Expected event name")?;

        // Process each event in the batch
        for i in 1..batch_array.len() {
            let event_params = &batch_array[i];
            match parse_single_event(event_name, event_params) {
                Ok(event) => events.push(event),
                Err(e) => {
                    warn!("Failed to parse event {}: {}", event_name, e);
                }
            }
        }
    }

    Ok(events)
}

/// Parse a single redraw event
fn parse_single_event(name: &str, params: &Value) -> Result<RedrawEvent, String> {
    let params_array = params.as_array().ok_or("Expected params array")?;

    match name {
        "grid_line" => {
            // [grid, row, col_start, cells]
            let grid = params_array.get(0)
                .and_then(|v| v.as_u64())
                .ok_or("Missing grid")?;
            let row = params_array.get(1)
                .and_then(|v| v.as_u64())
                .ok_or("Missing row")?;
            let col_start = params_array.get(2)
                .and_then(|v| v.as_u64())
                .ok_or("Missing col_start")?;
            let cells_data = params_array.get(3)
                .and_then(|v| v.as_array())
                .ok_or("Missing cells")?;

            let mut cells = Vec::new();
            for cell_data in cells_data {
                let cell_array = cell_data.as_array().ok_or("Expected cell array")?;
                let text = cell_array.get(0)
                    .and_then(|v| v.as_str())
                    .ok_or("Missing cell text")?;
                let hl_id = cell_array.get(1).and_then(|v| v.as_u64());
                let repeat = cell_array.get(2).and_then(|v| v.as_u64()).unwrap_or(1);

                cells.push(GridCell {
                    text: text.to_string(),
                    hl_id,
                    repeat,
                });
            }

            Ok(RedrawEvent::GridLine { grid, row, col_start, cells })
        }
        "grid_scroll" => {
            // [grid, top, bot, left, right, rows, cols]
            let grid = params_array.get(0).and_then(|v| v.as_u64()).ok_or("Missing grid")?;
            let top = params_array.get(1).and_then(|v| v.as_i64()).ok_or("Missing top")?;
            let bottom = params_array.get(2).and_then(|v| v.as_i64()).ok_or("Missing bottom")?;
            let left = params_array.get(3).and_then(|v| v.as_i64()).ok_or("Missing left")?;
            let right = params_array.get(4).and_then(|v| v.as_i64()).ok_or("Missing right")?;
            let rows = params_array.get(5).and_then(|v| v.as_i64()).ok_or("Missing rows")?;
            let cols = params_array.get(6).and_then(|v| v.as_i64()).unwrap_or(0);

            Ok(RedrawEvent::GridScroll { grid, top, bottom, left, right, rows, cols })
        }
        "grid_resize" => {
            // [grid, width, height]
            let grid = params_array.get(0).and_then(|v| v.as_u64()).ok_or("Missing grid")?;
            let width = params_array.get(1).and_then(|v| v.as_u64()).ok_or("Missing width")?;
            let height = params_array.get(2).and_then(|v| v.as_u64()).ok_or("Missing height")?;

            Ok(RedrawEvent::GridResize { grid, width, height })
        }
        "grid_clear" => {
            let grid = params_array.get(0).and_then(|v| v.as_u64()).ok_or("Missing grid")?;
            Ok(RedrawEvent::GridClear { grid })
        }
        "grid_cursor_goto" => {
            // [grid, row, col]
            let grid = params_array.get(0).and_then(|v| v.as_u64()).ok_or("Missing grid")?;
            let row = params_array.get(1).and_then(|v| v.as_u64()).ok_or("Missing row")?;
            let col = params_array.get(2).and_then(|v| v.as_u64()).ok_or("Missing col")?;

            Ok(RedrawEvent::GridCursorGoto { grid, row, col })
        }
        "default_colors_set" => {
            // [fg, bg, sp, cterm_fg, cterm_bg]
            let fg = params_array.get(0).and_then(|v| v.as_i64()).map(|c| parse_color(c as u32));
            let bg = params_array.get(1).and_then(|v| v.as_i64()).map(|c| parse_color(c as u32));
            let sp = params_array.get(2).and_then(|v| v.as_i64()).map(|c| parse_color(c as u32));

            Ok(RedrawEvent::DefaultColorsSet { fg, bg, sp })
        }
        "hl_attr_define" => {
            // [id, rgb_attrs, cterm_attrs, info]
            let id = params_array.get(0).and_then(|v| v.as_u64()).ok_or("Missing id")?;
            let rgb_attrs = params_array.get(1).and_then(|v| v.as_map());

            let attrs = if let Some(map) = rgb_attrs {
                parse_highlight_attrs(map)
            } else {
                HighlightAttrs::default()
            };

            Ok(RedrawEvent::HlAttrDefine { id, attrs })
        }
        "flush" => {
            Ok(RedrawEvent::Flush)
        }
        other => {
            Ok(RedrawEvent::Other(other.to_string()))
        }
    }
}

/// Parse RGB color from integer
fn parse_color(color: u32) -> Rgb {
    Rgb::new(
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
    )
}

/// Parse highlight attributes from map
fn parse_highlight_attrs(map: &[(Value, Value)]) -> HighlightAttrs {
    let mut attrs = HighlightAttrs::default();

    for (key, value) in map {
        if let Some(key_str) = key.as_str() {
            match key_str {
                "foreground" => {
                    if let Some(color) = value.as_u64() {
                        attrs.foreground = Some(parse_color(color as u32));
                    }
                }
                "background" => {
                    if let Some(color) = value.as_u64() {
                        attrs.background = Some(parse_color(color as u32));
                    }
                }
                "special" => {
                    if let Some(color) = value.as_u64() {
                        attrs.special = Some(parse_color(color as u32));
                    }
                }
                "reverse" => attrs.reverse = value.as_bool().unwrap_or(false),
                "italic" => attrs.italic = value.as_bool().unwrap_or(false),
                "bold" => attrs.bold = value.as_bool().unwrap_or(false),
                "strikethrough" => attrs.strikethrough = value.as_bool().unwrap_or(false),
                "underline" => attrs.underline = value.as_bool().unwrap_or(false),
                "undercurl" => attrs.undercurl = value.as_bool().unwrap_or(false),
                "blend" => {
                    if let Some(blend) = value.as_u64() {
                        attrs.blend = Some(blend as u8);
                    }
                }
                _ => {}
            }
        }
    }

    attrs
}