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
        info!("Attaching UI to Neovim ({}x{})", self.width, self.height);

        // Build nvim_ui_attach request
        let request = vec![
            Value::Integer(0.into()), // Message type: request
            Value::Integer(self.next_request_id.into()),
            Value::String("nvim_ui_attach".into()),
            Value::Array(vec![
                Value::Integer(self.width.into()),
                Value::Integer(self.height.into()),
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

    /// Send input to Neovim
    pub fn input(&mut self, input: &str) -> Result<(), String> {
        eprintln!("🔥 NVIM Sending input: {:?}", input);

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

        let request = vec![
            Value::Integer(0.into()),
            Value::Integer(self.next_request_id.into()),
            Value::String("nvim_ui_try_resize".into()),
            Value::Array(vec![
                Value::Integer(width.into()),
                Value::Integer(height.into()),
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