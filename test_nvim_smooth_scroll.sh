#!/bin/bash

# Test script for acvim smooth scrolling in Alacride

echo "Testing Alacride with Neovim smooth scrolling..."
echo ""
echo "This will:"
echo "  1. Launch Alacride in nvim mode"
echo "  2. Open a test file with lots of lines"
echo "  3. You can test smooth scrolling with j/k or Ctrl-D/Ctrl-U"
echo ""
echo "Expected behavior:"
echo "  - Scrolling should be buttery smooth (animated)"
echo "  - No jarring jumps between lines"
echo "  - Momentum scrolling if using trackpad"
echo ""

# Create a test file with many lines
TEST_FILE="/tmp/nvim_scroll_test.txt"
echo "Creating test file with 1000 lines..."
seq 1 1000 | awk '{print "Line " $1 ": The quick brown fox jumps over the lazy dog"}' > "$TEST_FILE"

echo "Launching: ./target/debug/alacritty --nvim-mode"
echo ""

# Launch Alacride with nvim mode
./target/debug/alacritty --nvim-mode "$TEST_FILE" &

ALACRITTY_PID=$!

echo "Alacritty launched with PID $ALACRITTY_PID"
echo ""
echo "Try these commands in Neovim to test smooth scrolling:"
echo "  - Press 'j' or 'k' to scroll one line (should animate smoothly)"
echo "  - Press Ctrl-D or Ctrl-U to scroll half page (should animate)"
echo "  - Use trackpad/mouse wheel for momentum scrolling"
echo "  - Type ':q' to quit"
echo ""
echo "Watch for smooth animated scrolling behavior!"