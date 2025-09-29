// Simple fragment shader for fullscreen quad texture blitting
// Used by the offscreen compositor for smooth scrolling

in vec2 vTexCoord;
out vec4 fragColor;

// Offscreen texture containing pre-rendered terminal content
uniform sampler2D offscreenTexture;

void main() {
    // Sample from the offscreen texture with smooth scrolling offset
    // The texture contains 2x viewport height of pre-rendered content
    fragColor = texture(offscreenTexture, vTexCoord);
}