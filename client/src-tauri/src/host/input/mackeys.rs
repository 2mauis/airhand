//! W3C `KeyboardEvent.code` → macOS 虚拟键码（`HIToolbox/Events.h` 的 `kVK_*`）。
//!
//! 虚拟键码对应的是**物理按键位置**（与 Windows 扫描码、Linux evdev 键码一样），
//! 远端的键盘布局和输入法照常工作。

/// 修饰键：在事件的 flags 里体现，按下 / 抬起时系统生成 FlagsChanged 事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Control,
    Option,
    Command,
    Function,
}

pub fn lookup(code: &str) -> Option<u16> {
    let k = match code {
        "KeyA" => 0x00,
        "KeyS" => 0x01,
        "KeyD" => 0x02,
        "KeyF" => 0x03,
        "KeyH" => 0x04,
        "KeyG" => 0x05,
        "KeyZ" => 0x06,
        "KeyX" => 0x07,
        "KeyC" => 0x08,
        "KeyV" => 0x09,
        "IntlBackslash" => 0x0A, // kVK_ISO_Section
        "KeyB" => 0x0B,
        "KeyQ" => 0x0C,
        "KeyW" => 0x0D,
        "KeyE" => 0x0E,
        "KeyR" => 0x0F,
        "KeyY" => 0x10,
        "KeyT" => 0x11,
        "Digit1" => 0x12,
        "Digit2" => 0x13,
        "Digit3" => 0x14,
        "Digit4" => 0x15,
        "Digit6" => 0x16,
        "Digit5" => 0x17,
        "Equal" => 0x18,
        "Digit9" => 0x19,
        "Digit7" => 0x1A,
        "Minus" => 0x1B,
        "Digit8" => 0x1C,
        "Digit0" => 0x1D,
        "BracketRight" => 0x1E,
        "KeyO" => 0x1F,
        "KeyU" => 0x20,
        "BracketLeft" => 0x21,
        "KeyI" => 0x22,
        "KeyP" => 0x23,
        "Enter" => 0x24,
        "KeyL" => 0x25,
        "KeyJ" => 0x26,
        "Quote" => 0x27,
        "KeyK" => 0x28,
        "Semicolon" => 0x29,
        "Backslash" => 0x2A,
        "Comma" => 0x2B,
        "Slash" => 0x2C,
        "KeyN" => 0x2D,
        "KeyM" => 0x2E,
        "Period" => 0x2F,
        "Tab" => 0x30,
        "Space" => 0x31,
        "Backquote" => 0x32,
        "Backspace" => 0x33, // kVK_Delete
        "Escape" => 0x35,
        "MetaRight" | "OSRight" => 0x36,
        "MetaLeft" | "OSLeft" => 0x37,
        "ShiftLeft" => 0x38,
        "CapsLock" => 0x39,
        "AltLeft" => 0x3A,
        "ControlLeft" => 0x3B,
        "ShiftRight" => 0x3C,
        "AltRight" => 0x3D,
        "ControlRight" => 0x3E,
        "Fn" => 0x3F,
        "F17" => 0x40,
        "NumpadDecimal" => 0x41,
        "NumpadMultiply" => 0x43,
        "NumpadAdd" => 0x45,
        "NumLock" => 0x47, // kVK_ANSI_KeypadClear（Mac 键盘在 NumLock 的位置上）
        "AudioVolumeUp" => 0x48,
        "AudioVolumeDown" => 0x49,
        "AudioVolumeMute" => 0x4A,
        "NumpadDivide" => 0x4B,
        "NumpadEnter" => 0x4C,
        "NumpadSubtract" => 0x4E,
        "F18" => 0x4F,
        "F19" => 0x50,
        "NumpadEqual" => 0x51,
        "Numpad0" => 0x52,
        "Numpad1" => 0x53,
        "Numpad2" => 0x54,
        "Numpad3" => 0x55,
        "Numpad4" => 0x56,
        "Numpad5" => 0x57,
        "Numpad6" => 0x58,
        "Numpad7" => 0x59,
        "F20" => 0x5A,
        "Numpad8" => 0x5B,
        "Numpad9" => 0x5C,
        "IntlYen" => 0x5D,
        "IntlRo" => 0x5E,
        "NumpadComma" => 0x5F,
        "F5" => 0x60,
        "F6" => 0x61,
        "F7" => 0x62,
        "F3" => 0x63,
        "F8" => 0x64,
        "F9" => 0x65,
        "Lang2" => 0x66, // kVK_JIS_Eisu（英数）
        "F11" => 0x67,
        "Lang1" => 0x68, // kVK_JIS_Kana（かな）
        "F13" | "PrintScreen" => 0x69,
        "F16" => 0x6A,
        "F14" | "ScrollLock" => 0x6B,
        "F10" => 0x6D,
        "ContextMenu" => 0x6E,
        "F12" => 0x6F,
        "F15" | "Pause" => 0x71,
        "Insert" | "Help" => 0x72, // kVK_Help（Mac 键盘在 Insert 的位置上）
        "Home" => 0x73,
        "PageUp" => 0x74,
        "Delete" => 0x75, // kVK_ForwardDelete
        "F4" => 0x76,
        "End" => 0x77,
        "F2" => 0x78,
        "PageDown" => 0x79,
        "F1" => 0x7A,
        "ArrowLeft" => 0x7B,
        "ArrowRight" => 0x7C,
        "ArrowDown" => 0x7D,
        "ArrowUp" => 0x7E,
        _ => return None,
    };
    Some(k)
}

/// 修饰键的种类和左右（`true` = 右侧）。
pub fn modifier(keycode: u16) -> Option<(Modifier, bool)> {
    Some(match keycode {
        0x38 => (Modifier::Shift, false),
        0x3C => (Modifier::Shift, true),
        0x3B => (Modifier::Control, false),
        0x3E => (Modifier::Control, true),
        0x3A => (Modifier::Option, false),
        0x3D => (Modifier::Option, true),
        0x37 => (Modifier::Command, false),
        0x36 => (Modifier::Command, true),
        0x3F => (Modifier::Function, false),
        _ => return None,
    })
}

/// 真实键盘上这些键的事件带有"数字小键盘" / "功能键"标志，部分程序据此区分按键。
pub fn extra_flags(keycode: u16) -> u64 {
    const NUMERIC_PAD: u64 = 0x0020_0000;
    const SECONDARY_FN: u64 = 0x0080_0000;
    match keycode {
        // 方向键
        0x7B..=0x7E => NUMERIC_PAD | SECONDARY_FN,
        // 小键盘
        0x41 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4C | 0x4E | 0x51..=0x59 | 0x5B | 0x5C => NUMERIC_PAD,
        // F 键、Home/End/PageUp/PageDown、向前删除、Help
        0x40
        | 0x4F
        | 0x50
        | 0x5A
        | 0x60..=0x65
        | 0x67
        | 0x69..=0x6B
        | 0x6D
        | 0x6F
        | 0x71..=0x7A => SECONDARY_FN,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys() {
        assert_eq!(lookup("KeyA"), Some(0x00));
        assert_eq!(lookup("KeyZ"), Some(0x06));
        assert_eq!(lookup("Enter"), Some(0x24));
        assert_eq!(lookup("Backspace"), Some(0x33));
        assert_eq!(lookup("Delete"), Some(0x75));
        assert_eq!(lookup("MetaLeft"), Some(0x37));
        assert_eq!(lookup("ArrowUp"), Some(0x7E));
        assert_eq!(lookup("Unidentified"), None);
        assert_eq!(modifier(0x37), Some((Modifier::Command, false)));
        assert_eq!(modifier(0x00), None);
        assert_eq!(extra_flags(0x7E) & 0x0020_0000, 0x0020_0000);
        assert_eq!(extra_flags(0x00), 0);
    }

    /// 不同的 code 不能映射到同一个键码（别名除外）
    #[test]
    fn no_collisions() {
        let codes = [
            "KeyA",
            "KeyB",
            "KeyC",
            "KeyD",
            "KeyE",
            "KeyF",
            "KeyG",
            "KeyH",
            "KeyI",
            "KeyJ",
            "KeyK",
            "KeyL",
            "KeyM",
            "KeyN",
            "KeyO",
            "KeyP",
            "KeyQ",
            "KeyR",
            "KeyS",
            "KeyT",
            "KeyU",
            "KeyV",
            "KeyW",
            "KeyX",
            "KeyY",
            "KeyZ",
            "Digit0",
            "Digit1",
            "Digit2",
            "Digit3",
            "Digit4",
            "Digit5",
            "Digit6",
            "Digit7",
            "Digit8",
            "Digit9",
            "Minus",
            "Equal",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Semicolon",
            "Quote",
            "Backquote",
            "Comma",
            "Period",
            "Slash",
            "Enter",
            "Tab",
            "Space",
            "Backspace",
            "Escape",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "ShiftLeft",
            "ShiftRight",
            "ControlLeft",
            "ControlRight",
            "AltLeft",
            "AltRight",
            "MetaLeft",
            "MetaRight",
            "CapsLock",
            "F1",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "F10",
            "F11",
            "F12",
            "Numpad0",
            "Numpad1",
            "Numpad2",
            "Numpad3",
            "Numpad4",
            "Numpad5",
            "Numpad6",
            "Numpad7",
            "Numpad8",
            "Numpad9",
            "NumpadAdd",
            "NumpadSubtract",
            "NumpadMultiply",
            "NumpadDivide",
            "NumpadDecimal",
            "NumpadEnter",
            "IntlBackslash",
        ];
        let mut seen = std::collections::HashMap::new();
        for c in codes {
            let k = lookup(c).unwrap_or_else(|| panic!("{c} 没有映射"));
            if let Some(prev) = seen.insert(k, c) {
                panic!("{c} 和 {prev} 都映射到 {k:#x}");
            }
        }
    }
}
