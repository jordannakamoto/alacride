//! Neovim External UI Integration
//!
//! This module implements a Neovim external UI client that allows Alacride
//! to act as a native rendering frontend for Neovim, enabling smooth scrolling
//! and GPU-accelerated rendering of Neovim buffers.
//!
//! Architecture:
//! - Spawns `nvim --embed` as a subprocess
//! - Communicates via MessagePack-RPC over stdin/stdout
//! - Receives UI events (grid_line, grid_scroll, etc.)
//! - Translates events to Alacride's rendering system
//! - Integrates with smooth scroll renderer for buttery animations

/// Enable debug logging for Neovim UI (set to false to disable 🔥 logs)
pub const NVIM_DEBUG: bool = false;

/// Debug macro - only prints if NVIM_DEBUG is enabled
/// Use this instead of eprintln! for all Neovim-related debug logs
#[macro_export]
macro_rules! nvim_debug {
    ($($arg:tt)*) => {
        if $crate::nvim_ui::NVIM_DEBUG {
            eprintln!($($arg)*);
        }
    };
}

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use log::{debug, error, info, warn};
use rmpv::Value;

mod protocol;
mod grid;
mod renderer_bridge;
mod mode;
pub mod input;

pub use grid::{Grid, GridCell};
pub use protocol::{NvimEvent, NvimRequest, NvimResponse, RedrawEvent};
pub use renderer_bridge::NvimRendererBridge;
pub use mode::NvimMode;

/// Neovim UI client that manages the embedded Neovim instance
pub struct NvimClient {
    /// Child process handle
    child: Child,
    /// Stdin writer
    stdin: ChildStdin,
    /// Event receiver (from reader thread)
    event_rx: Receiver<NvimEvent>,
    /// Request ID counter
    next_request_id: u64,
    /// UI dimensions
    width: u32,
    height: u32,
}

impl NvimClient {
    /// Spawn a new embedded Neovim instance
    pub fn spawn(width: u32, height: u32) -> Result<Self, String> {
        info!("Spawning embedded Neovim instance ({}x{})", width, height);

        // Spawn nvim with --embed flag
        let mut child = Command::new("nvim")
            .arg("--embed")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("Failed to spawn nvim: {}", e))?;

        let stdin = child.stdin.take().ok_or("Failed to open nvim stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to open nvim stdout")?;

        // Create channel for events
        let (event_tx, event_rx) = channel();

        // Spawn reader thread to process Neovim output
        thread::spawn(move || {
            Self::reader_thread(stdout, event_tx);
        });

        let mut client = Self {
            child,
            stdin,
            event_rx,
            next_request_id: 1,
            width,
            height,
        };

        // Attach UI to Neovim
        client.attach_ui()?;

        // Open sample file if it exists - use input to send ex command
        if std::path::Path::new("sample.txt").exists() {
            // Wait a bit for UI to be ready
            std::thread::sleep(std::time::Duration::from_millis(100));
            // Send :e command followed by Enter
            client.input(":e sample.txt\n")?;
        }

        Ok(client)
    }

    /// Reader thread that processes Neovim stdout
    fn reader_thread(stdout: ChildStdout, event_tx: Sender<NvimEvent>) {
        let mut reader = BufReader::new(stdout);
        loop {
            match rmpv::decode::read_value(&mut reader) {
                Ok(value) => {
                    match Self::parse_message(&value) {
                        Ok(event) => {
                            if event_tx.send(event).is_err() {
                                debug!("Event receiver dropped, stopping reader thread");
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse Neovim message: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to read from Neovim: {}", e);
                    break;
                }
            }
        }
    }

    /// Parse a MessagePack-RPC message from Neovim
    fn parse_message(value: &Value) -> Result<NvimEvent, String> {
        let array = value.as_array().ok_or("Expected array")?;
        if array.is_empty() {
            return Err("Empty message array".to_string());
        }

        let msg_type = array[0]
            .as_u64()
            .ok_or("Invalid message type")?;

        match msg_type {
            2 => {
                // Notification
                if array.len() < 3 {
                    return Err("Invalid notification format".to_string());
                }
                let method = array[1]
                    .as_str()
                    .ok_or("Invalid method name")?;
                let params = array[2].clone();

                protocol::parse_notification(method, params)
            }
            1 => {
                // Response
                Ok(NvimEvent::Response(NvimResponse {
                    id: array[1].as_u64().unwrap_or(0),
                    error: array[2].clone(),
                    result: array.get(3).cloned(),
                }))
            }
            0 => {
                // Request (server -> client)
                Ok(NvimEvent::Request(NvimRequest {
                    id: array[1].as_u64().unwrap_or(0),
                    method: array[2].as_str().unwrap_or("").to_string(),
                    params: array.get(3).cloned().unwrap_or(Value::Nil),
                }))
            }
            _ => Err(format!("Unknown message type: {}", msg_type)),
        }
    }

    /// Attach UI to Neovim
    fn attach_ui(&mut self) -> Result<(), String> {
        // First, disable statusline and cmdline to maximize usable space
        self.send_command("set laststatus=0")?;  // Disable status line
        self.send_command("set cmdheight=0")?;    // Disable command line
        self.send_command("set number")?;         // Enable line numbers for boundary detection
        self.send_command("set fillchars=eob:\\ ")?;  // Hide tildes at end of buffer

        // Add buffer lines for smooth scrolling (1 above, 1 below)
        let buffer_height = self.height + 2;
        info!("Attaching UI to Neovim ({}x{} with {} buffer height)", self.width, self.height, buffer_height);

        // Build nvim_ui_attach request
        let request = vec![
            Value::Integer(0.into()), // Message type: request
            Value::Integer(self.next_request_id.into()),
            Value::String("nvim_ui_attach".into()),
            Value::Array(vec![
                Value::Integer(self.width.into()),
                Value::Integer(buffer_height.into()),
                Value::Map(vec![
                    (
                        Value::String("rgb".into()),
                        Value::Boolean(true),
                    ),
                    (
                        Value::String("ext_linegrid".into()),
                        Value::Boolean(true),
                    ),
                    (
                        Value::String("ext_multigrid".into()),
                        Value::Boolean(false),
                    ),
                ]),
            ]),
        ];

        self.next_request_id += 1;

        // Serialize and send
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &Value::Array(request))
            .map_err(|e| format!("Failed to encode request: {}", e))?;

        self.stdin.write_all(&buf)
            .map_err(|e| format!("Failed to write to nvim: {}", e))?;
        self.stdin.flush()
            .map_err(|e| format!("Failed to flush stdin: {}", e))?;

        debug!("UI attach request sent");
        Ok(())
    }

    /// Send a command to Neovim
    fn send_command(&mut self, command: &str) -> Result<(), String> {
        let request = vec![
            Value::Integer(0.into()),
            Value::Integer(self.next_request_id.into()),
            Value::String("nvim_command".into()),
            Value::Array(vec![Value::String(command.into())]),
        ];

        self.next_request_id += 1;

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &Value::Array(request))
            .map_err(|e| format!("Failed to encode command: {}", e))?;

        self.stdin.write_all(&buf)
            .map_err(|e| format!("Failed to write command: {}", e))?;
        self.stdin.flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;

        Ok(())
    }

    /// Send input to Neovim
    pub fn input(&mut self, input: &str) -> Result<(), String> {
        nvim_debug!("🔥 NVIM Sending input: {:?}", input);

        let request = vec![
            Value::Integer(0.into()),
            Value::Integer(self.next_request_id.into()),
            Value::String("nvim_input".into()),
            Value::Array(vec![Value::String(input.into())]),
        ];

        self.next_request_id += 1;

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &Value::Array(request))
            .map_err(|e| format!("Failed to encode input: {}", e))?;

        self.stdin.write_all(&buf)
            .map_err(|e| format!("Failed to write input: {}", e))?;
        self.stdin.flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;

        Ok(())
    }

    /// Evaluate a Vim expression (returns request ID for tracking response)
    pub fn eval_expr(&mut self, expr: &str) -> Result<u64, String> {
        let request_id = self.next_request_id;

        let request = vec![
            Value::Integer(0.into()),
            Value::Integer(request_id.into()),
            Value::String("nvim_eval".into()),
            Value::Array(vec![Value::String(expr.into())]),
        ];

        self.next_request_id += 1;

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &Value::Array(request))
            .map_err(|e| format!("Failed to encode eval: {}", e))?;

        self.stdin.write_all(&buf)
            .map_err(|e| format!("Failed to write eval: {}", e))?;
        self.stdin.flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;

        Ok(request_id)
    }

    /// Poll for events from Neovim
    pub fn poll_events(&mut self) -> Vec<NvimEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Resize the UI
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.width = width;
        self.height = height;

        // Add buffer lines for smooth scrolling
        let buffer_height = height + 2;

        let request = vec![
            Value::Integer(0.into()),
            Value::Integer(self.next_request_id.into()),
            Value::String("nvim_ui_try_resize".into()),
            Value::Array(vec![
                Value::Integer(width.into()),
                Value::Integer(buffer_height.into()),
            ]),
        ];

        self.next_request_id += 1;

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &Value::Array(request))
            .map_err(|e| format!("Failed to encode resize: {}", e))?;

        self.stdin.write_all(&buf)
            .map_err(|e| format!("Failed to write resize: {}", e))?;
        self.stdin.flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;

        Ok(())
    }
}

impl Drop for NvimClient {
    fn drop(&mut self) {
        info!("Shutting down Neovim instance");
        let _ = self.child.kill();
    }
}