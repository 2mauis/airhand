//! W3C `KeyboardEvent.code` → Linux evdev 键码（`linux/input-event-codes.h`）。
//!
//! 与 Windows 用扫描码注入一样，这里按**物理按键**注入：远端的键盘布局和输入法照常工作。
//! X11 的键码 = evdev 键码 + 8（XKB evdev 规则，现代 Linux 发行版都是这一套）。

pub fn lookup(code: &str) -> Option<u16> {
    let k = match code {
        "Escape" => 1,
        "Digit1" => 2,
        "Digit2" => 3,
        "Digit3" => 4,
        "Digit4" => 5,
        "Digit5" => 6,
        "Digit6" => 7,
        "Digit7" => 8,
        "Digit8" => 9,
        "Digit9" => 10,
        "Digit0" => 11,
        "Minus" => 12,
        "Equal" => 13,
        "Backspace" => 14,
        "Tab" => 15,
        "KeyQ" => 16,
        "KeyW" => 17,
        "KeyE" => 18,
        "KeyR" => 19,
        "KeyT" => 20,
        "KeyY" => 21,
        "KeyU" => 22,
        "KeyI" => 23,
        "KeyO" => 24,
        "KeyP" => 25,
        "BracketLeft" => 26,
        "BracketRight" => 27,
        "Enter" => 28,
        "ControlLeft" => 29,
        "KeyA" => 30,
        "KeyS" => 31,
        "KeyD" => 32,
        "KeyF" => 33,
        "KeyG" => 34,
        "KeyH" => 35,
        "KeyJ" => 36,
        "KeyK" => 37,
        "KeyL" => 38,
        "Semicolon" => 39,
        "Quote" => 40,
        "Backquote" => 41,
        "ShiftLeft" => 42,
        "Backslash" => 43,
        "KeyZ" => 44,
        "KeyX" => 45,
        "KeyC" => 46,
        "KeyV" => 47,
        "KeyB" => 48,
        "KeyN" => 49,
        "KeyM" => 50,
        "Comma" => 51,
        "Period" => 52,
        "Slash" => 53,
        "ShiftRight" => 54,
        "NumpadMultiply" => 55,
        "AltLeft" => 56,
        "Space" => 57,
        "CapsLock" => 58,
        "F1" => 59,
        "F2" => 60,
        "F3" => 61,
        "F4" => 62,
        "F5" => 63,
        "F6" => 64,
        "F7" => 65,
        "F8" => 66,
        "F9" => 67,
        "F10" => 68,
        "NumLock" => 69,
        "ScrollLock" => 70,
        "Numpad7" => 71,
        "Numpad8" => 72,
        "Numpad9" => 73,
        "NumpadSubtract" => 74,
        "Numpad4" => 75,
        "Numpad5" => 76,
        "Numpad6" => 77,
        "NumpadAdd" => 78,
        "Numpad1" => 79,
        "Numpad2" => 80,
        "Numpad3" => 81,
        "Numpad0" => 82,
        "NumpadDecimal" => 83,
        "Lang5" => 85,         // KEY_ZENKAKUHANKAKU
        "IntlBackslash" => 86, // KEY_102ND
        "F11" => 87,
        "F12" => 88,
        "IntlRo" => 89,     // KEY_RO
        "Lang3" => 90,      // KEY_KATAKANA
        "Lang4" => 91,      // KEY_HIRAGANA
        "Convert" => 92,    // KEY_HENKAN
        "KanaMode" => 93,   // KEY_KATAKANAHIRAGANA
        "NonConvert" => 94, // KEY_MUHENKAN
        "NumpadEnter" => 96,
        "ControlRight" => 97,
        "NumpadDivide" => 98,
        "PrintScreen" => 99, // KEY_SYSRQ
        "AltRight" => 100,
        "Home" => 102,
        "ArrowUp" => 103,
        "PageUp" => 104,
        "ArrowLeft" => 105,
        "ArrowRight" => 106,
        "End" => 107,
        "ArrowDown" => 108,
        "PageDown" => 109,
        "Insert" => 110,
        "Delete" => 111,
        "AudioVolumeMute" => 113,
        "AudioVolumeDown" => 114,
        "AudioVolumeUp" => 115,
        "Power" => 116,
        "NumpadEqual" => 117,
        "Pause" => 119,
        "NumpadComma" => 121,
        "Lang1" => 122, // KEY_HANGEUL
        "Lang2" => 123, // KEY_HANJA
        "IntlYen" => 124,
        "MetaLeft" | "OSLeft" => 125,
        "MetaRight" | "OSRight" => 126,
        "ContextMenu" => 127, // KEY_COMPOSE
        "BrowserStop" => 128,
        "Again" => 129,
        "Props" => 130,
        "Undo" => 131,
        "Select" => 132, // KEY_FRONT
        "Copy" => 133,
        "Open" => 134,
        "Paste" => 135,
        "Find" => 136,
        "Cut" => 137,
        "Help" => 138,
        "LaunchApp2" => 140, // KEY_CALC
        "Sleep" => 142,
        "WakeUp" => 143,
        "LaunchApp1" => 144, // KEY_FILE（我的电脑）
        "LaunchMail" => 155,
        "BrowserFavorites" => 156,
        "BrowserBack" => 158,
        "BrowserForward" => 159,
        "Eject" => 161,
        "MediaTrackNext" => 163,
        "MediaPlayPause" => 164,
        "MediaTrackPrevious" => 165,
        "MediaStop" => 166,
        "BrowserHome" => 172,
        "BrowserRefresh" => 173,
        "F13" => 183,
        "F14" => 184,
        "F15" => 185,
        "F16" => 186,
        "F17" => 187,
        "F18" => 188,
        "F19" => 189,
        "F20" => 190,
        "F21" => 191,
        "F22" => 192,
        "F23" => 193,
        "F24" => 194,
        "BrowserSearch" => 217,
        "MediaSelect" => 226, // KEY_MEDIA
        _ => return None,
    };
    Some(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys() {
        assert_eq!(lookup("KeyA"), Some(30));
        assert_eq!(lookup("Enter"), Some(28));
        assert_eq!(lookup("ArrowUp"), Some(103));
        assert_eq!(lookup("MetaLeft"), Some(125));
        assert_eq!(lookup("Unidentified"), None);
        // X11 键码只有一个字节
        for c in ["F24", "BrowserSearch", "MediaSelect"] {
            assert!(lookup(c).unwrap() + 8 <= 255);
        }
    }
}
