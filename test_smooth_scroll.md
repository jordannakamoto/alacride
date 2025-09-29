# Testing Alacride Smooth Scrolling with Neovim

## Setup
```bash
./target/debug/alacritty --nvim-mode
```

## What Triggers Smooth Scrolling

### ✅ WILL Trigger Smooth Scrolling:
1. **Mouse wheel** - Now sends `<C-y>` and `<C-e>` to Neovim
2. **Ctrl-D / Ctrl-U** - Half-page scroll (if they trigger grid_scroll)
3. **Ctrl-Y / Ctrl-E** - Line-by-line viewport scroll
4. **zt, zz, zb** - Reposition commands that scroll viewport

### ❌ WON'T Trigger Smooth Scrolling:
- **j / k keys** - These move the cursor, not viewport
  - Viewport only scrolls when cursor moves off-screen
  - This is normal Neovim behavior

## How to Test

1. Open a file with many lines:
   ```bash
   ./target/debug/alacritty --nvim-mode /tmp/test_file.txt
   ```

2. Try these commands:
   - **Mouse wheel** - Should animate smoothly now
   - **Ctrl-D** - Scroll down half page (should animate)
   - **Ctrl-U** - Scroll up half page (should animate)
   - **Ctrl-E** - Scroll down one line (should animate)
   - **Ctrl-Y** - Scroll up one line (should animate)

3. Watch the debug output in terminal to see:
   ```
   🔥 NVIM Sending input: "<C-e>"
   🔥 NVIM Processing X events
   🔥 NVIM Found GridScroll event!
   🔥 NVIM GridScroll: rows=X
   🔥 NVIM Applying smooth scroll delta: X
   ```

## Expected Behavior

- Mouse wheel scrolling should now work
- Scroll commands should animate smoothly
- Regular cursor movement (j/k) won't animate (this is correct)

## Debug

If you still don't see animation, check:
1. Are GridScroll events being received? (check debug output)
2. Is `renderer.update_smooth_scroll()` being called?
3. Is `pixel_offset` non-zero during rendering?