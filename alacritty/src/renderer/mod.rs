use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{fmt, ptr};

use ahash::RandomState;
use crossfont::Metrics;
use glutin::context::{ContextApi, GlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use log::{LevelFilter, debug, info};
use unicode_width::UnicodeWidthChar;

use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::Flags;

use crate::config::debug::{Debug as DebugConfig, RendererPreference};
use crate::display::SizeInfo;
use crate::display::color::Rgb;
use crate::display::content::RenderableCell;
use crate::gl;
use crate::gl::types::{GLfloat, GLint, GLsizeiptr, GLuint};
use crate::renderer::rects::{RectRenderer, RenderRect};
use crate::renderer::shader::{ShaderError, ShaderProgram};

pub mod platform;
pub mod rects;
mod shader;
mod text;

pub use text::{GlyphCache, LoaderApi};

use shader::ShaderVersion;
use text::{Gles2Renderer, Glsl3Renderer, TextRenderer};

// Shaders for offscreen compositor texture blitting
const BLIT_SHADER_V: &str = include_str!("../../res/glsl3/blit.v.glsl");
const BLIT_SHADER_F: &str = include_str!("../../res/glsl3/blit.f.glsl");

/// Whether the OpenGL functions have been loaded.
pub static GL_FUNS_LOADED: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub enum Error {
    /// Shader error.
    Shader(ShaderError),

    /// Other error.
    Other(String),
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Shader(err) => err.source(),
            Error::Other(_) => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Shader(err) => {
                write!(f, "There was an error initializing the shaders: {err}")
            },
            Error::Other(err) => {
                write!(f, "{err}")
            },
        }
    }
}

impl From<ShaderError> for Error {
    fn from(val: ShaderError) -> Self {
        Error::Shader(val)
    }
}

impl From<String> for Error {
    fn from(val: String) -> Self {
        Error::Other(val)
    }
}

#[derive(Debug)]
enum TextRendererProvider {
    Gles2(Gles2Renderer),
    Glsl3(Glsl3Renderer),
}

/// Offscreen compositor for smooth scrolling without terminal grid updates
///
/// This system creates a virtual scrollable texture that's larger than the viewport
/// (typically 2x viewport height) and renders terminal content to it. The compositor
/// then blits from this offscreen texture to the screen with smooth pixel-level offsets,
/// creating buttery smooth scrolling without needing to update the terminal grid
/// every frame.
///
/// Key benefits:
/// - Smooth scrolling without line pop-in artifacts
/// - GPU-accelerated compositing for performance
/// - Decouples visual scrolling from terminal content updates
/// - Similar to how modern web browsers handle smooth scrolling
#[derive(Debug)]
struct OffscreenCompositor {
    /// OpenGL framebuffer object for offscreen rendering
    fbo: GLuint,
    /// Color texture attached to the framebuffer (holds rendered terminal content)
    texture: GLuint,
    /// Depth renderbuffer (may not be needed for terminal rendering, but good practice)
    depth_buffer: GLuint,
    /// Width of offscreen buffer (matches viewport width)
    width: i32,
    /// Height of offscreen buffer (typically 2x viewport height for smooth scrolling)
    height: i32,
    /// Current virtual scroll offset within the offscreen buffer (in pixels)
    /// This tracks where we are in the virtual scrollable space
    virtual_offset: f32,
    /// Last terminal display_offset when the offscreen buffer was last updated
    /// Used to determine when we need to refresh the offscreen content
    last_display_offset: usize,
    /// Whether the compositor has been properly initialized
    initialized: bool,
}

impl OffscreenCompositor {
    /// Create new offscreen compositor (uninitialized)
    fn new() -> Self {
        Self {
            fbo: 0,
            texture: 0,
            depth_buffer: 0,
            width: 0,
            height: 0,
            virtual_offset: 0.0,
            last_display_offset: 0,
            initialized: false,
        }
    }

    /// Initialize or resize the offscreen framebuffer
    ///
    /// Creates an offscreen rendering target that's larger than the viewport
    /// to support smooth scrolling. The buffer is sized as:
    /// - Width: matches viewport width exactly
    /// - Height: 2x viewport height to provide scroll buffer above/below
    fn resize(&mut self, viewport_width: i32, viewport_height: i32) -> Result<(), Error> {
        unsafe {
            // Clean up existing OpenGL objects if they exist
            self.cleanup_gl_objects();

            // Create larger offscreen buffer for smooth scrolling
            // Using 2x height provides buffer space above and below current viewport
            self.width = viewport_width;
            self.height = viewport_height * 2;

            // Create and configure framebuffer object (FBO)
            gl::GenFramebuffers(1, &mut self.fbo);
            gl::BindFramebuffer(gl::FRAMEBUFFER, self.fbo);

            // Create color texture to hold rendered terminal content
            gl::GenTextures(1, &mut self.texture);
            gl::BindTexture(gl::TEXTURE_2D, self.texture);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA as i32,
                self.width,
                self.height,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                ptr::null(),
            );

            // Configure texture filtering for smooth scaling
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);

            // Attach texture as color buffer
            gl::FramebufferTexture2D(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                self.texture,
                0,
            );

            // Create depth buffer (may not be essential for terminal rendering)
            gl::GenRenderbuffers(1, &mut self.depth_buffer);
            gl::BindRenderbuffer(gl::RENDERBUFFER, self.depth_buffer);
            gl::RenderbufferStorage(gl::RENDERBUFFER, gl::DEPTH_COMPONENT, self.width, self.height);
            gl::FramebufferRenderbuffer(
                gl::FRAMEBUFFER,
                gl::DEPTH_ATTACHMENT,
                gl::RENDERBUFFER,
                self.depth_buffer,
            );

            // Verify framebuffer is complete and ready for rendering
            let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
            if status != gl::FRAMEBUFFER_COMPLETE {
                self.cleanup_gl_objects();
                return Err(Error::Other(format!(
                    "Offscreen framebuffer incomplete: status = 0x{:x}",
                    status
                )));
            }

            // Restore default framebuffer
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);

            self.initialized = true;
            debug!("Offscreen compositor initialized: {}x{}", self.width, self.height);
        }

        Ok(())
    }

    /// Bind the offscreen framebuffer for rendering
    /// All subsequent draw calls will render to the offscreen texture
    fn bind_for_rendering(&self) {
        if !self.initialized {
            return;
        }

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, self.fbo);
            gl::Viewport(0, 0, self.width, self.height);
        }
    }

    /// Bind the default framebuffer (screen) for rendering
    fn bind_default_framebuffer(&self) {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        }
    }

    /// Check if the offscreen content needs to be updated
    ///
    /// The offscreen buffer should be refreshed when:
    /// 1. Terminal display offset has changed significantly (new content visible)
    /// 2. We've scrolled far enough that we're approaching the buffer edges
    /// 3. Terminal content has changed (handled externally)
    fn needs_update(&self, display_offset: usize, scroll_offset: f32, _cell_height: f32) -> bool {
        if !self.initialized {
            return true;
        }

        // Update if display offset changed significantly
        // This catches cases where user jumped to different parts of history
        let offset_threshold = 10; // lines
        let offset_changed =
            (display_offset as i32 - self.last_display_offset as i32).abs() > offset_threshold;

        // Update if we've scrolled close to the buffer boundaries
        // Keep content centered in the offscreen buffer for maximum scroll range
        let buffer_quarter = (self.height as f32) * 0.25;
        let scroll_near_edge = scroll_offset.abs() > buffer_quarter;

        offset_changed || scroll_near_edge
    }

    /// Update tracking information after refreshing offscreen content
    fn mark_updated(&mut self, display_offset: usize, scroll_offset: f32) {
        self.last_display_offset = display_offset;
        self.virtual_offset = scroll_offset;
    }

    /// Clean up OpenGL objects (called on resize or drop)
    unsafe fn cleanup_gl_objects(&mut self) {
        unsafe {
            if self.fbo != 0 {
                gl::DeleteFramebuffers(1, &self.fbo);
                self.fbo = 0;
            }
            if self.texture != 0 {
                gl::DeleteTextures(1, &self.texture);
                self.texture = 0;
            }
            if self.depth_buffer != 0 {
                gl::DeleteRenderbuffers(1, &self.depth_buffer);
                self.depth_buffer = 0;
            }
        }
        self.initialized = false;
    }

    /// Get the texture handle for compositing to screen
    fn texture_handle(&self) -> GLuint {
        self.texture
    }

    /// Check if compositor is ready for use
    fn is_initialized(&self) -> bool {
        self.initialized
    }
}

impl Drop for OffscreenCompositor {
    fn drop(&mut self) {
        unsafe {
            self.cleanup_gl_objects();
        }
    }
}

/// Simple fullscreen quad renderer for texture blitting
///
/// This renderer draws a fullscreen quad with a texture, used by the offscreen
/// compositor to blit the pre-rendered terminal content to the screen with smooth
/// scroll offsets. It's essentially a simple 2D texture renderer optimized for
/// the compositor use case.
#[derive(Debug)]
struct QuadRenderer {
    /// Shader program for texture blitting
    shader: Option<BlitShaderProgram>,
    /// Vertex Array Object
    vao: GLuint,
    /// Vertex Buffer Object for quad vertices
    vbo: GLuint,
    /// Element Buffer Object for quad indices
    ebo: GLuint,
    /// Whether the renderer is initialized
    initialized: bool,
}

impl QuadRenderer {
    /// Create new quad renderer (uninitialized)
    fn new() -> Self {
        Self { shader: None, vao: 0, vbo: 0, ebo: 0, initialized: false }
    }

    /// Initialize the quad renderer with OpenGL resources
    fn initialize(&mut self) -> Result<(), Error> {
        unsafe {
            // Create shader program
            let shader = BlitShaderProgram::new()?;

            // Create fullscreen quad vertices
            // Position (NDC: -1 to 1) and texture coordinates (0 to 1)
            #[rustfmt::skip]
            let vertices: [GLfloat; 16] = [
                // Position   TexCoord
                -1.0, -1.0,   0.0, 0.0,  // Bottom-left
                 1.0, -1.0,   1.0, 0.0,  // Bottom-right
                 1.0,  1.0,   1.0, 1.0,  // Top-right
                -1.0,  1.0,   0.0, 1.0,  // Top-left
            ];

            // Quad indices (two triangles)
            let indices: [u32; 6] = [
                0, 1, 2, // First triangle
                2, 3, 0, // Second triangle
            ];

            // Generate and setup VAO
            gl::GenVertexArrays(1, &mut self.vao);
            gl::BindVertexArray(self.vao);

            // Generate and setup VBO
            gl::GenBuffers(1, &mut self.vbo);
            gl::BindBuffer(gl::ARRAY_BUFFER, self.vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                (vertices.len() * std::mem::size_of::<GLfloat>()) as GLsizeiptr,
                vertices.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );

            // Generate and setup EBO
            gl::GenBuffers(1, &mut self.ebo);
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.ebo);
            gl::BufferData(
                gl::ELEMENT_ARRAY_BUFFER,
                (indices.len() * std::mem::size_of::<u32>()) as GLsizeiptr,
                indices.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );

            // Setup vertex attributes
            // Position attribute (location = 0)
            gl::VertexAttribPointer(
                0,
                2,
                gl::FLOAT,
                gl::FALSE,
                (4 * std::mem::size_of::<GLfloat>()) as GLint,
                std::ptr::null(),
            );
            gl::EnableVertexAttribArray(0);

            // Texture coordinate attribute (location = 1)
            gl::VertexAttribPointer(
                1,
                2,
                gl::FLOAT,
                gl::FALSE,
                (4 * std::mem::size_of::<GLfloat>()) as GLint,
                (2 * std::mem::size_of::<GLfloat>()) as *const _,
            );
            gl::EnableVertexAttribArray(1);

            // Unbind VAO
            gl::BindVertexArray(0);

            self.shader = Some(shader);
            self.initialized = true;

            debug!("QuadRenderer initialized successfully");
        }

        Ok(())
    }

    /// Render a fullscreen quad with the given texture and scroll offset
    fn render(&self, texture: GLuint, scroll_offset: f32) {
        if !self.initialized {
            return;
        }

        let shader = self.shader.as_ref().unwrap();

        unsafe {
            // Use the blit shader program
            shader.use_program();

            // Bind the offscreen texture
            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindTexture(gl::TEXTURE_2D, texture);
            shader.set_texture(0);

            // Set the scroll offset uniform
            shader.set_scroll_offset(scroll_offset);

            // Render the fullscreen quad
            gl::BindVertexArray(self.vao);
            gl::DrawElements(gl::TRIANGLES, 6, gl::UNSIGNED_INT, std::ptr::null());
            gl::BindVertexArray(0);
        }
    }

    /// Clean up OpenGL resources
    unsafe fn cleanup(&mut self) {
        unsafe {
            if self.vao != 0 {
                gl::DeleteVertexArrays(1, &self.vao);
                self.vao = 0;
            }
            if self.vbo != 0 {
                gl::DeleteBuffers(1, &self.vbo);
                self.vbo = 0;
            }
            if self.ebo != 0 {
                gl::DeleteBuffers(1, &self.ebo);
                self.ebo = 0;
            }
        }
        self.shader = None;
        self.initialized = false;
    }
}

impl Drop for QuadRenderer {
    fn drop(&mut self) {
        unsafe {
            self.cleanup();
        }
    }
}

/// Shader program for texture blitting
#[derive(Debug)]
struct BlitShaderProgram {
    program: ShaderProgram,
    u_texture: GLint,
    u_scroll_offset: GLint,
}

impl BlitShaderProgram {
    fn new() -> Result<Self, Error> {
        let program = ShaderProgram::new(ShaderVersion::Glsl3, None, BLIT_SHADER_V, BLIT_SHADER_F)?;

        let u_texture = program.get_uniform_location(c"offscreenTexture")?;
        let u_scroll_offset = program.get_uniform_location(c"scrollOffset")?;

        Ok(Self { program, u_texture, u_scroll_offset })
    }

    fn use_program(&self) {
        unsafe {
            gl::UseProgram(self.program.id());
        }
    }

    fn set_texture(&self, texture_unit: i32) {
        unsafe {
            gl::Uniform1i(self.u_texture, texture_unit);
        }
    }

    fn set_scroll_offset(&self, offset: f32) {
        unsafe {
            gl::Uniform1f(self.u_scroll_offset, offset);
        }
    }
}

#[derive(Debug)]
pub struct Renderer {
    text_renderer: TextRendererProvider,
    rect_renderer: RectRenderer,
    /// Offscreen compositor for smooth scrolling without terminal grid updates
    offscreen_compositor: OffscreenCompositor,
    /// Quad renderer for texture blitting (used by offscreen compositor)
    quad_renderer: QuadRenderer,
    /// Simple smooth-scroll residual in pixels (no momentum). Always in [-cell_height, cell_height).
    simple_scroll_residual: f32,
    /// Simple momentum velocity in pixels per second.
    simple_scroll_velocity: f32,
    /// NEW: Direct scroll state
    direct_scroll_total_px: f32,
    is_in_momentum_scroll: bool,
    /// Cached cell height in pixels (from font metrics).
    cell_height_px: f32,
    /// Timestamp of last momentum advance.
    last_smooth_ts: Option<Instant>,
    /// Timestamp of last input delta to distinguish active scroll input.
    last_input_ts: Option<Instant>,
    /// Timestamp when the current scroll gesture started (for initial acceleration ramp).
    gesture_start_ts: Option<Instant>,
    /// Last input direction (-1.0, 0.0, 1.0) to handle direction changes.
    last_input_dir: f32,
    /// Terminal bounds for scroll limiting
    terminal_screen_lines: usize,
    terminal_history_size: usize,
    terminal_display_offset: usize,
    robustness: bool,
    /// Debug flag for smooth scroll logging
    smooth_scroll_debug: bool,
}

/// Wrapper around gl::GetString with error checking and reporting.
fn gl_get_string(
    string_id: gl::types::GLenum,
    description: &str,
) -> Result<Cow<'static, str>, Error> {
    unsafe {
        let string_ptr = gl::GetString(string_id);
        match gl::GetError() {
            gl::NO_ERROR if !string_ptr.is_null() => {
                Ok(CStr::from_ptr(string_ptr as *const _).to_string_lossy())
            },
            gl::INVALID_ENUM => {
                Err(format!("OpenGL error requesting {description}: invalid enum").into())
            },
            error_id => Err(format!("OpenGL error {error_id} requesting {description}").into()),
        }
    }
}

impl Renderer {
    /// Create a new renderer.
    ///
    /// This will automatically pick between the GLES2 and GLSL3 renderer based on the GPU's
    /// supported OpenGL version.
    pub fn new(
        context: &PossiblyCurrentContext,
        debug_config: &DebugConfig,
    ) -> Result<Self, Error> {
        // We need to load OpenGL functions once per instance, but only after we make our context
        // current due to WGL limitations.
        if !GL_FUNS_LOADED.swap(true, Ordering::Relaxed) {
            let gl_display = context.display();
            gl::load_with(|symbol| {
                let symbol = CString::new(symbol).unwrap();
                gl_display.get_proc_address(symbol.as_c_str()).cast()
            });
        }

        let shader_version = gl_get_string(gl::SHADING_LANGUAGE_VERSION, "shader version")?;
        let gl_version = gl_get_string(gl::VERSION, "OpenGL version")?;
        let renderer = gl_get_string(gl::RENDERER, "renderer version")?;

        info!("Running on {renderer}");
        info!("OpenGL version {gl_version}, shader_version {shader_version}");

        // Check if robustness is supported.
        let robustness = Self::supports_robustness();

        let is_gles_context = matches!(context.context_api(), ContextApi::Gles(_));

        // Use the config option to enforce a particular renderer configuration.
        let (use_glsl3, allow_dsb) = match debug_config.renderer {
            Some(RendererPreference::Glsl3) => (true, true),
            Some(RendererPreference::Gles2) => (false, true),
            Some(RendererPreference::Gles2Pure) => (false, false),
            None => (shader_version.as_ref() >= "3.3" && !is_gles_context, true),
        };

        let (text_renderer, rect_renderer) = if use_glsl3 {
            let text_renderer = TextRendererProvider::Glsl3(Glsl3Renderer::new()?);
            let rect_renderer = RectRenderer::new(ShaderVersion::Glsl3)?;
            (text_renderer, rect_renderer)
        } else {
            let text_renderer =
                TextRendererProvider::Gles2(Gles2Renderer::new(allow_dsb, is_gles_context)?);
            let rect_renderer = RectRenderer::new(ShaderVersion::Gles2)?;
            (text_renderer, rect_renderer)
        };

        // Enable debug logging for OpenGL as well.
        if log::max_level() >= LevelFilter::Debug && GlExtensions::contains("GL_KHR_debug") {
            debug!("Enabled debug logging for OpenGL");
            unsafe {
                gl::Enable(gl::DEBUG_OUTPUT);
                gl::Enable(gl::DEBUG_OUTPUT_SYNCHRONOUS);
                gl::DebugMessageCallback(Some(gl_debug_log), ptr::null_mut());
            }
        }

        Ok(Self {
            text_renderer,
            rect_renderer,
            offscreen_compositor: OffscreenCompositor::new(),
            quad_renderer: QuadRenderer::new(),
            simple_scroll_residual: 0.0,
            simple_scroll_velocity: 0.0,
            direct_scroll_total_px: 0.0,
            is_in_momentum_scroll: false,
            cell_height_px: 0.0,
            last_smooth_ts: None,
            last_input_ts: None,
            gesture_start_ts: None,
            last_input_dir: 0.0,
            terminal_screen_lines: 0,
            terminal_history_size: 0,
            terminal_display_offset: 0,
            robustness,
            smooth_scroll_debug: debug_config.smooth_scroll_debug,
        })
    }

    pub fn draw_cells<I: Iterator<Item = RenderableCell>>(
        &mut self,
        size_info: &SizeInfo,
        glyph_cache: &mut GlyphCache,
        cells: I,
    ) {
        match &mut self.text_renderer {
            TextRendererProvider::Gles2(renderer) => {
                renderer.draw_cells(size_info, glyph_cache, cells)
            },
            TextRendererProvider::Glsl3(renderer) => {
                renderer.draw_cells(size_info, glyph_cache, cells)
            },
        }
    }

    /// Draw cells using offscreen compositor for smooth scrolling
    ///
    /// This method implements a two-stage rendering approach:
    /// 1. Render terminal content to an offscreen texture (2x viewport height)
    /// 2. Composite the offscreen texture to screen with smooth pixel offset
    ///
    /// The offscreen texture is only updated when necessary (significant scrolling
    /// or content changes), making scrolling smooth without expensive re-renders.
    pub fn draw_cells_smooth<I: Iterator<Item = RenderableCell>>(
        &mut self,
        size_info: &SizeInfo,
        glyph_cache: &mut GlyphCache,
        cells: I,
        pixel_offset: f32,
    ) {
        // For now, fall back to direct rendering until we implement the compositor fully
        // TODO: Implement full offscreen compositor rendering pipeline

        // TEMPORARY: Disable offscreen compositor - use fallback path
        if true || !self.offscreen_compositor.is_initialized() || !self.quad_renderer.initialized {
            // Fallback: use existing smooth scroll system
            log::trace!("Offscreen compositor fallback path active");
            self.draw_cells_smooth_fallback(size_info, glyph_cache, cells, pixel_offset);
            return;
        }

        // DEBUG: Log that we're using the offscreen compositor
        log::trace!("Using offscreen compositor for smooth scrolling");

        // Check if we need to update the offscreen content
        // This happens when scrolling far or when content changes significantly
        let cell_height = size_info.cell_height();
        if self.offscreen_compositor.needs_update(0, pixel_offset, cell_height) {
            // Render to offscreen texture
            self.render_to_offscreen(size_info, glyph_cache, cells);
            self.offscreen_compositor.mark_updated(0, pixel_offset);
        }

        // Composite offscreen texture to screen with smooth offset
        self.composite_offscreen_to_screen(size_info, pixel_offset);
    }

    /// Fallback smooth rendering (uses existing system)
    fn draw_cells_smooth_fallback<I: Iterator<Item = RenderableCell>>(
        &mut self,
        size_info: &SizeInfo,
        glyph_cache: &mut GlyphCache,
        cells: I,
        pixel_offset: f32,
    ) {
        let adjusted_cells: Vec<_> = cells.collect();

        match &mut self.text_renderer {
            TextRendererProvider::Gles2(renderer) => renderer.draw_cells_with_offset(
                size_info,
                glyph_cache,
                adjusted_cells.into_iter(),
                pixel_offset,
            ),
            TextRendererProvider::Glsl3(renderer) => renderer.draw_cells_with_offset(
                size_info,
                glyph_cache,
                adjusted_cells.into_iter(),
                pixel_offset,
            ),
        }
    }

    /// Render terminal content to the offscreen texture
    fn render_to_offscreen<I: Iterator<Item = RenderableCell>>(
        &mut self,
        size_info: &SizeInfo,
        glyph_cache: &mut GlyphCache,
        cells: I,
    ) {
        // Bind offscreen framebuffer for rendering
        self.offscreen_compositor.bind_for_rendering();

        // Clear the offscreen buffer
        unsafe {
            gl::ClearColor(0.0, 0.0, 0.0, 1.0); // Clear to black
            gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
        }

        // Render available cells to offscreen texture
        // NOTE: We only have viewport cells available, so the offscreen buffer will have
        // the same line pop-in issue until we implement expanded cell collection.
        // However, the compositor infrastructure is now in place for future improvement.
        let adjusted_cells: Vec<_> = cells.collect();

        match &mut self.text_renderer {
            TextRendererProvider::Gles2(renderer) => renderer.draw_cells_with_offset(
                size_info,
                glyph_cache,
                adjusted_cells.into_iter(),
                0.0,
            ),
            TextRendererProvider::Glsl3(renderer) => renderer.draw_cells_with_offset(
                size_info,
                glyph_cache,
                adjusted_cells.into_iter(),
                0.0,
            ),
        }

        // Restore default framebuffer
        self.offscreen_compositor.bind_default_framebuffer();
    }

    /// Composite the offscreen texture to the screen with smooth offset
    fn composite_offscreen_to_screen(&self, size_info: &SizeInfo, pixel_offset: f32) {
        // Restore viewport for screen rendering
        self.set_viewport(size_info);

        if !self.quad_renderer.initialized {
            return;
        }

        // Calculate texture coordinate offset based on pixel offset
        // The offscreen texture is 2x viewport height, so we need to normalize the offset
        let viewport_height = size_info.height() as f32;
        let texture_height = viewport_height * 2.0;

        // Convert pixel offset to texture coordinate offset (0.0 to 1.0 range)
        // Positive pixel_offset (scrolling down) moves texture up to reveal content below
        let texture_offset = pixel_offset / texture_height;

        // Center the viewport in the middle of the 2x texture (0.25 to 0.75 range normally)
        let centered_offset = 0.25 + texture_offset; // Start at 1/4 into texture

        // Clear the screen
        unsafe {
            gl::Clear(gl::COLOR_BUFFER_BIT);
        }

        // Disable depth testing for fullscreen quad
        unsafe {
            gl::Disable(gl::DEPTH_TEST);
        }

        // Render fullscreen quad with offscreen texture
        self.quad_renderer.render(self.offscreen_compositor.texture_handle(), centered_offset);

        // Re-enable depth testing
        unsafe {
            gl::Enable(gl::DEPTH_TEST);
        }
    }

    /// Draw a string in a variable location. Used for printing the render timer, warnings and
    /// errors.
    pub fn draw_string(
        &mut self,
        point: Point<usize>,
        fg: Rgb,
        bg: Rgb,
        string_chars: impl Iterator<Item = char>,
        size_info: &SizeInfo,
        glyph_cache: &mut GlyphCache,
    ) {
        let mut wide_char_spacer = false;
        let cells = string_chars.enumerate().filter_map(|(i, character)| {
            let flags = if wide_char_spacer {
                wide_char_spacer = false;
                return None;
            } else if character.width() == Some(2) {
                // The spacer is always following the wide char.
                wide_char_spacer = true;
                Flags::WIDE_CHAR
            } else {
                Flags::empty()
            };

            Some(RenderableCell {
                point: Point::new(point.line, point.column + i),
                character,
                extra: None,
                flags,
                bg_alpha: 1.0,
                fg,
                bg,
                underline: fg,
            })
        });

        self.draw_cells(size_info, glyph_cache, cells);
    }

    pub fn with_loader<F, T>(&mut self, func: F) -> T
    where
        F: FnOnce(LoaderApi<'_>) -> T,
    {
        match &mut self.text_renderer {
            TextRendererProvider::Gles2(renderer) => renderer.with_loader(func),
            TextRendererProvider::Glsl3(renderer) => renderer.with_loader(func),
        }
    }

    /// Draw all rectangles simultaneously to prevent excessive program swaps.
    pub fn draw_rects(&mut self, size_info: &SizeInfo, metrics: &Metrics, rects: Vec<RenderRect>) {
        if rects.is_empty() {
            return;
        }

        // Prepare rect rendering state.
        unsafe {
            // Remove padding from viewport.
            gl::Viewport(0, 0, size_info.width() as i32, size_info.height() as i32);
            gl::BlendFuncSeparate(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA, gl::SRC_ALPHA, gl::ONE);
        }

        self.rect_renderer.draw(size_info, metrics, rects);

        // Activate regular state again.
        unsafe {
            // Reset blending strategy.
            gl::BlendFunc(gl::SRC1_COLOR, gl::ONE_MINUS_SRC1_COLOR);

            // Restore viewport with padding.
            self.set_viewport(size_info);
        }
    }

    /// Fill the window with `color` and `alpha`.
    pub fn clear(&self, color: Rgb, alpha: f32) {
        unsafe {
            gl::ClearColor(
                (f32::from(color.r) / 255.0).min(1.0) * alpha,
                (f32::from(color.g) / 255.0).min(1.0) * alpha,
                (f32::from(color.b) / 255.0).min(1.0) * alpha,
                alpha,
            );
            gl::Clear(gl::COLOR_BUFFER_BIT);
        }
    }

    /// Get the context reset status.
    pub fn was_context_reset(&self) -> bool {
        // If robustness is not supported, don't use its functions.
        if !self.robustness {
            return false;
        }

        let status = unsafe { gl::GetGraphicsResetStatus() };
        if status == gl::NO_ERROR {
            false
        } else {
            let reason = match status {
                gl::GUILTY_CONTEXT_RESET_KHR => "guilty",
                gl::INNOCENT_CONTEXT_RESET_KHR => "innocent",
                gl::UNKNOWN_CONTEXT_RESET_KHR => "unknown",
                _ => "invalid",
            };

            info!("GPU reset ({reason})");

            true
        }
    }

    fn supports_robustness() -> bool {
        let mut notification_strategy = 0;
        if GlExtensions::contains("GL_KHR_robustness") {
            unsafe {
                gl::GetIntegerv(gl::RESET_NOTIFICATION_STRATEGY_KHR, &mut notification_strategy);
            }
        } else {
            notification_strategy = gl::NO_RESET_NOTIFICATION_KHR as gl::types::GLint;
        }

        if notification_strategy == gl::LOSE_CONTEXT_ON_RESET_KHR as gl::types::GLint {
            info!("GPU reset notifications are enabled");
            true
        } else {
            info!("GPU reset notifications are disabled");
            false
        }
    }

    pub fn finish(&self) {
        unsafe {
            gl::Finish();
        }
    }

    /// Update smooth scroll renderer with font metrics
    pub fn update_smooth_scroll_metrics(&mut self, metrics: &crossfont::Metrics) {
        self.cell_height_px = metrics.line_height as f32;
    }

    /// Update terminal bounds for smooth scroll renderer
    pub fn update_smooth_scroll_bounds(&mut self, screen_lines: usize, history_size: usize) {
        eprintln!("🔥 BOUNDS: Setting screen_lines={}, history_size={}", screen_lines, history_size);
        self.terminal_screen_lines = screen_lines;
        self.terminal_history_size = history_size;
        eprintln!("🔥 BOUNDS: After setting: terminal_screen_lines={}, terminal_history_size={}",
                  self.terminal_screen_lines, self.terminal_history_size);
    }

    /// Set the current terminal display offset
    pub fn set_display_offset(&mut self, display_offset: usize) {
        eprintln!("🔥 OFFSET: Setting display_offset={}", display_offset);
        self.terminal_display_offset = display_offset;
        eprintln!("🔥 OFFSET: After setting: terminal_display_offset={}", self.terminal_display_offset);
    }

    /// Update smooth scroll based on *pixel* delta (positive = scroll up).
    pub fn update_smooth_scroll_pixels(&mut self, pixel_delta: f32) {
        // Use macOS PixelDelta values directly without sensitivity adjustment
        // Natural scrolling on macOS usually reports positive up; Alacritty typically expects
        // "scroll up" to move the view *down* through history (i.e., reveal older lines).
        let delta = -pixel_delta;

        // Calculate current bounds in pixels
        let max_down_lines = self.terminal_display_offset;
        let max_up_lines = self.terminal_history_size.saturating_sub(self.terminal_display_offset);
        let max_up_px = (max_up_lines as f32) * self.cell_height_px;
        let max_down_px = (max_down_lines as f32) * self.cell_height_px;

        eprintln!("🔥 RENDERER_PIXELS: pixel_delta={}, delta={}", pixel_delta, delta);
        eprintln!("🔥 RENDERER_PIXELS: display_offset={}, history_size={}",
                  self.terminal_display_offset, self.terminal_history_size);
        eprintln!("🔥 RENDERER_PIXELS: max_up_px={}, max_down_px={}", max_up_px, max_down_px);
        eprintln!("🔥 RENDERER_PIXELS: current total={}", self.direct_scroll_total_px);

        let now = Instant::now();

        // Simplified: always use direct scroll mode for now to debug
        // TODO: Re-add momentum mode once basic scrolling works
        self.is_in_momentum_scroll = false;
        self.simple_scroll_velocity = 0.0;

        // Direct accumulation with bounds checking
        let potential_total = self.direct_scroll_total_px + delta;

        eprintln!("🔥 RENDERER_PIXELS: potential_total={}", potential_total);

        // Only accumulate if we're not at the boundaries
        if potential_total <= max_up_px && potential_total >= -max_down_px {
            eprintln!("🔥 RENDERER_PIXELS: ✅ ACCEPTING scroll");
            self.direct_scroll_total_px = potential_total;
        } else if potential_total > max_up_px {
            eprintln!("🔥 RENDERER_PIXELS: ❌ CLAMPED to max_up");
            self.direct_scroll_total_px = max_up_px;
        } else if potential_total < -max_down_px {
            eprintln!("🔥 RENDERER_PIXELS: ❌ CLAMPED to max_down");
            self.direct_scroll_total_px = -max_down_px;
        }

        self.simple_scroll_residual = self.direct_scroll_total_px;

        eprintln!("🔥 RENDERER_PIXELS: final residual={}", self.simple_scroll_residual);

        self.last_input_ts = Some(now);
    }

    /// Legacy line-based API for compatibility
    pub fn update_smooth_scroll(&mut self, line_delta: f32) {
        // Get cell height from size info during first render if not set
        if self.cell_height_px <= 0.0 {
            self.cell_height_px = 20.0; // Fallback, will be updated in advance_smooth_scroll
        }
        let pixel_delta = line_delta * self.cell_height_px;
        eprintln!("🔥 RENDERER update_smooth_scroll: line_delta={}, cell_height={}, pixel_delta={}",
                  line_delta, self.cell_height_px, pixel_delta);
        eprintln!("🔥 RENDERER before: residual={}, velocity={}",
                  self.simple_scroll_residual, self.simple_scroll_velocity);
        self.update_smooth_scroll_pixels(pixel_delta);
        eprintln!("🔥 RENDERER after: residual={}, velocity={}",
                  self.simple_scroll_residual, self.simple_scroll_velocity);
    }

    /// Check if smooth scroll/momentum is active
    pub fn is_smooth_scroll_animating(&self) -> bool {
        self.simple_scroll_velocity.abs() > 1.0 || self.simple_scroll_residual.abs() > 0.1
    }

    /// Advance animator for this frame, compute pixel_offset and normalize by consuming full-line
    /// offsets. Returns (pixel_offset, lines_to_scroll).
    pub fn advance_smooth_scroll(
        &mut self,
        size_info: &SizeInfo,
        max_down_lines: usize,
        max_up_lines: usize,
    ) -> (f32, i32) {
        let cell_h = size_info.cell_height();
        if cell_h <= 0.0 { return (0.0, 0); }
        self.cell_height_px = cell_h;

        let now = Instant::now();
        let mut lines_scrolled = 0;

        // Calculate bounds in pixels for both scroll directions
        let max_up_px = (max_up_lines as f32) * cell_h;
        let max_down_px = (max_down_lines as f32) * cell_h;

        if self.is_in_momentum_scroll {
            // --- ADVANCE MOMENTUM PHYSICS ---
            if let Some(prev) = self.last_smooth_ts {
                let dt = (now - prev).as_secs_f32();
                if dt > 0.0 && self.simple_scroll_velocity.abs() > 0.01 {
                    let potential_residual = self.simple_scroll_residual + self.simple_scroll_velocity * dt;

                    // Check bounds and stop momentum at edges
                    if potential_residual >= max_up_px && self.simple_scroll_velocity > 0.0 {
                        self.simple_scroll_residual = max_up_px;
                        self.simple_scroll_velocity = 0.0;
                        self.direct_scroll_total_px = max_up_px;
                    } else if potential_residual <= -max_down_px && self.simple_scroll_velocity < 0.0 {
                        self.simple_scroll_residual = -max_down_px;
                        self.simple_scroll_velocity = 0.0;
                        self.direct_scroll_total_px = -max_down_px;
                    } else {
                        self.simple_scroll_residual = potential_residual;
                        let friction = 0.92_f32;
                        self.simple_scroll_velocity *= friction.powf(dt * 60.0);
                    }
                }
            }
            // Use truncation instead of rounding to allow small movements
            lines_scrolled = (self.simple_scroll_residual / cell_h) as i32;
            if lines_scrolled != 0 {
                self.simple_scroll_residual -= (lines_scrolled as f32) * cell_h;
            }
            // If velocity becomes very small, transition back to direct mode.
            if self.simple_scroll_velocity.abs() < 0.5 {
                self.is_in_momentum_scroll = false;
                self.direct_scroll_total_px = self.simple_scroll_residual;
            }
        } else {
            // --- DIRECT PIXEL SCROLL MODE ---
            // Apply bounds to direct scroll accumulator
            if self.direct_scroll_total_px > max_up_px {
                self.direct_scroll_total_px = max_up_px;
            } else if self.direct_scroll_total_px < -max_down_px {
                self.direct_scroll_total_px = -max_down_px;
            }

            self.simple_scroll_residual = self.direct_scroll_total_px;

            // Convert to line scrolls when we have at least 1 full line worth of pixels
            // But keep the fractional pixel remainder for smooth visual offset
            lines_scrolled = (self.simple_scroll_residual / cell_h) as i32;

            // Clamp lines_scrolled to available bounds
            if lines_scrolled > 0 {
                lines_scrolled = lines_scrolled.min(max_up_lines as i32);
            } else if lines_scrolled < 0 {
                lines_scrolled = lines_scrolled.max(-(max_down_lines as i32));
            }

            if lines_scrolled != 0 {
                // Subtract the line portion, keep pixel remainder for smooth rendering
                self.direct_scroll_total_px -= (lines_scrolled as f32) * cell_h;
                self.simple_scroll_residual = self.direct_scroll_total_px;
            }
        }

        self.last_smooth_ts = Some(now);

        (self.simple_scroll_residual, lines_scrolled)
    }

    /// Stop momentum scrolling and optionally snap to the nearest line (residual=0).
    pub fn stop_smooth_scroll(&mut self, snap_to_line: bool) {
        self.simple_scroll_velocity = 0.0;
        if snap_to_line {
            self.simple_scroll_residual = 0.0;
        }
        let now = Instant::now();
        self.last_smooth_ts = Some(now);
        self.last_input_ts = Some(now);
        // Reset gesture so next deltas ramp up again.
        self.gesture_start_ts = Some(now);
        self.last_input_dir = 0.0;
    }

    /// Set Neovim scroll offset directly (bypasses bounds checking)
    /// This is used when Neovim has already scrolled the content and we just
    /// want to temporarily show it at the old position, then animate to 0
    pub fn set_nvim_scroll_offset(&mut self, pixel_offset: f32) {
        eprintln!("🔥 NVIM Setting scroll offset: {}", pixel_offset);
        self.simple_scroll_residual = pixel_offset;
        self.direct_scroll_total_px = pixel_offset;
    }

    /// Advance smooth scroll animation for Neovim (no line scrolling, pure pixel animation)
    pub fn advance_nvim_smooth_scroll(&mut self, dt: f32) -> f32 {
        // Faster exponential decay animation towards zero to minimize tearing
        // Using 0.75 instead of 0.85 for quicker animation
        let decay_factor = 0.75_f32.powf(dt * 60.0); // 60fps normalized

        // Animate towards zero
        self.simple_scroll_residual *= decay_factor;

        // Stop when close enough to zero
        if self.simple_scroll_residual.abs() < 0.1 {
            self.simple_scroll_residual = 0.0;
        }

        eprintln!("🔥 NVIM Scroll offset: {}", self.simple_scroll_residual);
        self.simple_scroll_residual
    }

    /// Check if Neovim smooth scroll is animating
    pub fn is_nvim_scroll_animating(&self) -> bool {
        self.simple_scroll_residual.abs() > 0.1
    }

    /// Set the viewport for cell rendering.
    #[inline]
    pub fn set_viewport(&self, size: &SizeInfo) {
        unsafe {
            gl::Viewport(
                size.padding_x() as i32,
                size.padding_y() as i32,
                size.width() as i32 - 2 * size.padding_x() as i32,
                size.height() as i32 - 2 * size.padding_y() as i32,
            );
        }
    }

    /// Resize the renderer and initialize offscreen compositor.
    pub fn resize(&mut self, size_info: &SizeInfo) {
        self.set_viewport(size_info);

        // Resize offscreen compositor for smooth scrolling
        let viewport_width = size_info.width() as i32;
        let viewport_height = size_info.height() as i32;

        // Use 2x buffer size for optimal smooth scrolling pre-rendering
        // Memory usage is reasonable: ~8MB per 1920x1080 terminal (RGBA texture)
        if let Err(e) = self.offscreen_compositor.resize(viewport_width, viewport_height * 2) {
            log::error!("Failed to resize offscreen compositor: {}", e);
        }

        // Initialize quad renderer once (shared geometry, minimal memory overhead)
        if !self.quad_renderer.initialized {
            if let Err(e) = self.quad_renderer.initialize() {
                log::error!("Failed to initialize quad renderer: {}", e);
            }
        }

        // Reset smooth scroll state on resize to avoid display corruption
        // Cell height may have changed, making current pixel offsets invalid
        self.stop_smooth_scroll(true);
        self.cell_height_px = size_info.cell_height();

        match &self.text_renderer {
            TextRendererProvider::Gles2(renderer) => renderer.resize(size_info),
            TextRendererProvider::Glsl3(renderer) => renderer.resize(size_info),
        }
    }
}

struct GlExtensions;

impl GlExtensions {
    /// Check if the given `extension` is supported.
    ///
    /// This function will lazily load OpenGL extensions.
    fn contains(extension: &str) -> bool {
        static OPENGL_EXTENSIONS: OnceLock<HashSet<&'static str, RandomState>> = OnceLock::new();

        OPENGL_EXTENSIONS.get_or_init(Self::load_extensions).contains(extension)
    }

    /// Load available OpenGL extensions.
    fn load_extensions() -> HashSet<&'static str, RandomState> {
        unsafe {
            let extensions = gl::GetString(gl::EXTENSIONS);

            if extensions.is_null() {
                let mut extensions_number = 0;
                gl::GetIntegerv(gl::NUM_EXTENSIONS, &mut extensions_number);

                (0..extensions_number as gl::types::GLuint)
                    .flat_map(|i| {
                        let extension = CStr::from_ptr(gl::GetStringi(gl::EXTENSIONS, i) as *mut _);
                        extension.to_str()
                    })
                    .collect()
            } else {
                match CStr::from_ptr(extensions as *mut _).to_str() {
                    Ok(ext) => ext.split_whitespace().collect(),
                    Err(_) => Default::default(),
                }
            }
        }
    }
}

extern "system" fn gl_debug_log(
    _: gl::types::GLenum,
    _: gl::types::GLenum,
    _: gl::types::GLuint,
    _: gl::types::GLenum,
    _: gl::types::GLsizei,
    msg: *const gl::types::GLchar,
    _: *mut std::os::raw::c_void,
) {
    let msg = unsafe { CStr::from_ptr(msg).to_string_lossy() };
    debug!("[gl_render] {msg}");
}
