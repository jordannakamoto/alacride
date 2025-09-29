//! Input translation for Neovim
//!
//! Converts Alacride keyboard/mouse events to Neovim input format

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// Convert a keyboard event to Neovim input string
pub fn key_to_nvim_input(key_event: &KeyEvent, mods: ModifiersState) -> Option<String> {
    if key_event.state != ElementState::Pressed {
        return None;
    }

    let mut input = String::new();
    let ctrl = mods.control_key();
    let shift = mods.shift_key();
    let alt = mods.alt_key();
    let super_key = mods.super_key();

    // Handle special keys
    match &key_event.logical_key {
        Key::Named(named) => {
            let nvim_key = match named {
                NamedKey::Enter => Some("CR"),
                NamedKey::Escape => Some("Esc"),
                NamedKey::Backspace => Some("BS"),
                NamedKey::Tab => Some("Tab"),
                NamedKey::Space => Some("Space"),
                NamedKey::ArrowUp => Some("Up"),
                NamedKey::ArrowDown => Some("Down"),
                NamedKey::ArrowLeft => Some("Left"),
                NamedKey::ArrowRight => Some("Right"),
                NamedKey::Home => Some("Home"),
                NamedKey::End => Some("End"),
                NamedKey::PageUp => Some("PageUp"),
                NamedKey::PageDown => Some("PageDown"),
                NamedKey::Insert => Some("Insert"),
                NamedKey::Delete => Some("Del"),
                NamedKey::F1 => Some("F1"),
                NamedKey::F2 => Some("F2"),
                NamedKey::F3 => Some("F3"),
                NamedKey::F4 => Some("F4"),
                NamedKey::F5 => Some("F5"),
                NamedKey::F6 => Some("F6"),
                NamedKey::F7 => Some("F7"),
                NamedKey::F8 => Some("F8"),
                NamedKey::F9 => Some("F9"),
                NamedKey::F10 => Some("F10"),
                NamedKey::F11 => Some("F11"),
                NamedKey::F12 => Some("F12"),
                _ => None,
            };

            if let Some(key_name) = nvim_key {
                // Build modifier string
                let mut mod_string = String::new();
                if ctrl {
                    mod_string.push_str("C-");
                }
                if shift && !matches!(named, NamedKey::Tab) {
                    // Shift is implicit for most special keys
                    mod_string.push_str("S-");
                }
                if alt {
                    mod_string.push_str("A-");
                }
                if super_key {
                    mod_string.push_str("D-");
                }

                if mod_string.is_empty() {
                    input.push_str(&format!("<{}>", key_name));
                } else {
                    input.push_str(&format!("<{}{}>", mod_string, key_name));
                }
            }
        }
        Key::Character(c) => {
            let char_str = c.as_str();

            if ctrl {
                // Handle Ctrl+key combinations
                if let Some(first_char) = char_str.chars().next() {
                    if first_char.is_ascii_alphabetic() {
                        // Ctrl+letter
                        input.push_str(&format!("<C-{}>", first_char.to_ascii_lowercase()));
                    } else if first_char == ' ' {
                        input.push_str("<C-Space>");
                    } else {
                        input.push_str(&format!("<C-{}>", char_str));
                    }
                }
            } else if alt {
                // Handle Alt+key combinations
                input.push_str(&format!("<A-{}>", char_str));
            } else if super_key {
                // Handle Super/Cmd+key combinations
                input.push_str(&format!("<D-{}>", char_str));
            } else {
                // Regular character input
                input.push_str(char_str);
            }
        }
        _ => {
            return None;
        }
    }

    if input.is_empty() {
        None
    } else {
        Some(input)
    }
}

/// Convert physical key code to Neovim input (fallback)
pub fn physical_key_to_nvim_input(
    key_code: PhysicalKey,
    mods: ModifiersState,
) -> Option<String> {
    if let PhysicalKey::Code(code) = key_code {
        let ctrl = mods.control_key();
        let shift = mods.shift_key();
        let alt = mods.alt_key();

        // Map physical keys that might not have logical equivalents
        let key_name = match code {
            KeyCode::Enter | KeyCode::NumpadEnter => "CR",
            KeyCode::Escape => "Esc",
            KeyCode::Backspace => "BS",
            KeyCode::Tab => "Tab",
            KeyCode::Space => "Space",
            _ => return None,
        };

        let mut mod_string = String::new();
        if ctrl {
            mod_string.push_str("C-");
        }
        if shift {
            mod_string.push_str("S-");
        }
        if alt {
            mod_string.push_str("A-");
        }

        if mod_string.is_empty() {
            Some(format!("<{}>", key_name))
        } else {
            Some(format!("<{}{}>", mod_string, key_name))
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_character() {
        let event = KeyEvent {
            state: ElementState::Pressed,
            logical_key: Key::Character("a".into()),
            physical_key: PhysicalKey::Code(KeyCode::KeyA),
            location: winit::keyboard::KeyLocation::Standard,
            repeat: false,
            text: None,
            platform_specific: Default::default(),
        };
        let result = key_to_nvim_input(&event, ModifiersState::empty());
        assert_eq!(result, Some("a".to_string()));
    }

    #[test]
    fn test_ctrl_key() {
        let event = KeyEvent {
            state: ElementState::Pressed,
            logical_key: Key::Character("c".into()),
            physical_key: PhysicalKey::Code(KeyCode::KeyC),
            location: winit::keyboard::KeyLocation::Standard,
            repeat: false,
            text: None,
            platform_specific: Default::default(),
        };
        let mut mods = ModifiersState::empty();
        mods.set(ModifiersState::CONTROL, true);
        let result = key_to_nvim_input(&event, mods);
        assert_eq!(result, Some("<C-c>".to_string()));
    }

    #[test]
    fn test_escape_key() {
        let event = KeyEvent {
            state: ElementState::Pressed,
            logical_key: Key::Named(NamedKey::Escape),
            physical_key: PhysicalKey::Code(KeyCode::Escape),
            location: winit::keyboard::KeyLocation::Standard,
            repeat: false,
            text: None,
            platform_specific: Default::default(),
        };
        let result = key_to_nvim_input(&event, ModifiersState::empty());
        assert_eq!(result, Some("<Esc>".to_string()));
    }
}