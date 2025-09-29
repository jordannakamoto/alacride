// Simple vertex shader for fullscreen quad texture blitting
// Used by the offscreen compositor for smooth scrolling

layout(location = 0) in vec2 position;
layout(location = 1) in vec2 texCoord;

out vec2 vTexCoord;

// Smooth scroll Y offset in texture coordinates (0.0 to 1.0)
uniform float scrollOffset;

void main() {
    // Pass through vertex position (already in NDC: -1 to 1)
    gl_Position = vec4(position, 0.0, 1.0);

    // Apply smooth scroll offset to texture coordinates
    // Positive scrollOffset moves texture up (revealing content below)
    vTexCoord = vec2(texCoord.x, texCoord.y + scrollOffset);
}