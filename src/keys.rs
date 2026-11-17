use anyhow::{Result, anyhow};

/// Tecla pelo código Linux (`input-event-codes.h`), igual nos dois sistemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key(pub u16);

impl Key {
    pub const ESC: Key = Key(1);
    pub const BACKSPACE: Key = Key(14);
    #[cfg(target_os = "linux")]
    pub const TAB: Key = Key(15);
    pub const ENTER: Key = Key(28);
    pub const LEFTCTRL: Key = Key(29);
    pub const LEFTSHIFT: Key = Key(42);
    pub const RIGHTSHIFT: Key = Key(54);
    pub const LEFTALT: Key = Key(56);
    pub const F9: Key = Key(67);
    pub const F10: Key = Key(68);
    pub const RIGHTCTRL: Key = Key(97);
    pub const RIGHTALT: Key = Key(100);
    pub const LEFTMETA: Key = Key(125);
    pub const RIGHTMETA: Key = Key(126);

    /// Virtual-key do Windows equivalente.
    #[cfg(any(windows, test))]
    pub fn vk(self) -> Option<u16> {
        KEYS.iter().find(|k| k.1 == self.0).map(|k| k.2)
    }

    /// Tecla do código virtual-key do Windows.
    #[cfg(any(windows, test))]
    pub fn from_vk(vk: u16) -> Option<Key> {
        KEYS.iter().find(|k| k.2 == vk).map(|k| Key(k.1))
    }
}

/// Nome no script, código Linux e virtual-key do Windows.
const KEYS: &[(&str, u16, u16)] = &[
    ("ESC", 1, 0x1B),
    ("1", 2, 0x31),
    ("2", 3, 0x32),
    ("3", 4, 0x33),
    ("4", 5, 0x34),
    ("5", 6, 0x35),
    ("6", 7, 0x36),
    ("7", 8, 0x37),
    ("8", 9, 0x38),
    ("9", 10, 0x39),
    ("0", 11, 0x30),
    ("MINUS", 12, 0xBD),
    ("EQUAL", 13, 0xBB),
    ("BACKSPACE", 14, 0x08),
    ("TAB", 15, 0x09),
    ("Q", 16, 0x51),
    ("W", 17, 0x57),
    ("E", 18, 0x45),
    ("R", 19, 0x52),
    ("T", 20, 0x54),
    ("Y", 21, 0x59),
    ("U", 22, 0x55),
    ("I", 23, 0x49),
    ("O", 24, 0x4F),
    ("P", 25, 0x50),
    ("LEFTBRACE", 26, 0xDB),
    ("RIGHTBRACE", 27, 0xDD),
    ("ENTER", 28, 0x0D),
    ("LEFTCTRL", 29, 0xA2),
    ("A", 30, 0x41),
    ("S", 31, 0x53),
    ("D", 32, 0x44),
    ("F", 33, 0x46),
    ("G", 34, 0x47),
    ("H", 35, 0x48),
    ("J", 36, 0x4A),
    ("K", 37, 0x4B),
    ("L", 38, 0x4C),
    ("SEMICOLON", 39, 0xBA),
    ("APOSTROPHE", 40, 0xDE),
    ("GRAVE", 41, 0xC0),
    ("LEFTSHIFT", 42, 0xA0),
    ("BACKSLASH", 43, 0xDC),
    ("Z", 44, 0x5A),
    ("X", 45, 0x58),
    ("C", 46, 0x43),
    ("V", 47, 0x56),
    ("B", 48, 0x42),
    ("N", 49, 0x4E),
    ("M", 50, 0x4D),
    ("COMMA", 51, 0xBC),
    ("DOT", 52, 0xBE),
    ("SLASH", 53, 0xBF),
    ("RIGHTSHIFT", 54, 0xA1),
    ("KPASTERISK", 55, 0x6A),
    ("LEFTALT", 56, 0xA4),
    ("SPACE", 57, 0x20),
    ("CAPSLOCK", 58, 0x14),
    ("F1", 59, 0x70),
    ("F2", 60, 0x71),
    ("F3", 61, 0x72),
    ("F4", 62, 0x73),
    ("F5", 63, 0x74),
    ("F6", 64, 0x75),
    ("F7", 65, 0x76),
    ("F8", 66, 0x77),
    ("F9", 67, 0x78),
    ("F10", 68, 0x79),
    ("NUMLOCK", 69, 0x90),
    ("SCROLLLOCK", 70, 0x91),
    ("KP7", 71, 0x67),
    ("KP8", 72, 0x68),
    ("KP9", 73, 0x69),
    ("KPMINUS", 74, 0x6D),
    ("KP4", 75, 0x64),
    ("KP5", 76, 0x65),
    ("KP6", 77, 0x66),
    ("KPPLUS", 78, 0x6B),
    ("KP1", 79, 0x61),
    ("KP2", 80, 0x62),
    ("KP3", 81, 0x63),
    ("KP0", 82, 0x60),
    ("KPDOT", 83, 0x6E),
    ("102ND", 86, 0xE2),
    ("F11", 87, 0x7A),
    ("F12", 88, 0x7B),
    ("KPENTER", 96, 0x0D),
    ("RIGHTCTRL", 97, 0xA3),
    ("KPSLASH", 98, 0x6F),
    ("SYSRQ", 99, 0x2C),
    ("RIGHTALT", 100, 0xA5),
    ("HOME", 102, 0x24),
    ("UP", 103, 0x26),
    ("PAGEUP", 104, 0x21),
    ("LEFT", 105, 0x25),
    ("RIGHT", 106, 0x27),
    ("END", 107, 0x23),
    ("DOWN", 108, 0x28),
    ("PAGEDOWN", 109, 0x22),
    ("INSERT", 110, 0x2D),
    ("DELETE", 111, 0x2E),
    ("MUTE", 113, 0xAD),
    ("VOLUMEDOWN", 114, 0xAE),
    ("VOLUMEUP", 115, 0xAF),
    ("PAUSE", 119, 0x13),
    ("LEFTMETA", 125, 0x5B),
    ("RIGHTMETA", 126, 0x5C),
    ("COMPOSE", 127, 0x5D),
    ("NEXTSONG", 163, 0xB0),
    ("PLAYPAUSE", 164, 0xB3),
    ("PREVIOUSSONG", 165, 0xB1),
    ("STOPCD", 166, 0xB2),
    ("F13", 183, 0x7C),
    ("F14", 184, 0x7D),
    ("F15", 185, 0x7E),
    ("F16", 186, 0x7F),
    ("F17", 187, 0x80),
    ("F18", 188, 0x81),
    ("F19", 189, 0x82),
    ("F20", 190, 0x83),
    ("F21", 191, 0x84),
    ("F22", 192, 0x85),
    ("F23", 193, 0x86),
    ("F24", 194, 0x87),
];

/// Apelidos aceitos no script para modificadores e teclas comuns.
const ALIASES: &[(&str, Key)] = &[
    ("SUPER", Key::LEFTMETA),
    ("META", Key::LEFTMETA),
    ("WIN", Key::LEFTMETA),
    ("CTRL", Key::LEFTCTRL),
    ("CONTROL", Key::LEFTCTRL),
    ("ALT", Key::LEFTALT),
    ("ALTGR", Key::RIGHTALT),
    ("SHIFT", Key::LEFTSHIFT),
    ("RSUPER", Key::RIGHTMETA),
    ("RCTRL", Key::RIGHTCTRL),
    ("RSHIFT", Key::RIGHTSHIFT),
    ("RETURN", Key::ENTER),
    ("ESCAPE", Key::ESC),
    ("DEL", Key(111)),
];

/// Teclas sem shift e com shift no layout US.
const US_LAYOUT: &[(Key, char, char)] = &[
    (Key(2), '1', '!'),
    (Key(3), '2', '@'),
    (Key(4), '3', '#'),
    (Key(5), '4', '$'),
    (Key(6), '5', '%'),
    (Key(7), '6', '^'),
    (Key(8), '7', '&'),
    (Key(9), '8', '*'),
    (Key(10), '9', '('),
    (Key(11), '0', ')'),
    (Key(12), '-', '_'),
    (Key(13), '=', '+'),
    (Key(26), '[', '{'),
    (Key(27), ']', '}'),
    (Key(43), '\\', '|'),
    (Key(39), ';', ':'),
    (Key(40), '\'', '"'),
    (Key(41), '`', '~'),
    (Key(51), ',', '<'),
    (Key(52), '.', '>'),
    (Key(53), '/', '?'),
    (Key(57), ' ', ' '),
    (Key(16), 'q', 'Q'),
    (Key(17), 'w', 'W'),
    (Key(18), 'e', 'E'),
    (Key(19), 'r', 'R'),
    (Key(20), 't', 'T'),
    (Key(21), 'y', 'Y'),
    (Key(22), 'u', 'U'),
    (Key(23), 'i', 'I'),
    (Key(24), 'o', 'O'),
    (Key(25), 'p', 'P'),
    (Key(30), 'a', 'A'),
    (Key(31), 's', 'S'),
    (Key(32), 'd', 'D'),
    (Key(33), 'f', 'F'),
    (Key(34), 'g', 'G'),
    (Key(35), 'h', 'H'),
    (Key(36), 'j', 'J'),
    (Key(37), 'k', 'K'),
    (Key(38), 'l', 'L'),
    (Key(44), 'z', 'Z'),
    (Key(45), 'x', 'X'),
    (Key(46), 'c', 'C'),
    (Key(47), 'v', 'V'),
    (Key(48), 'b', 'B'),
    (Key(49), 'n', 'N'),
    (Key(50), 'm', 'M'),
];

/// Modificadores reconhecidos na gravação.
pub const MODIFIERS: &[Key] = &[
    Key::LEFTCTRL,
    Key::RIGHTCTRL,
    Key::LEFTALT,
    Key::RIGHTALT,
    Key::LEFTMETA,
    Key::RIGHTMETA,
    Key::LEFTSHIFT,
    Key::RIGHTSHIFT,
];

/// A tecla é modificador?
pub fn is_modifier(key: Key) -> bool {
    MODIFIERS.contains(&key)
}

/// A tecla é shift?
pub fn is_shift(key: Key) -> bool {
    key == Key::LEFTSHIFT || key == Key::RIGHTSHIFT
}

/// Lê uma tecla pelo nome do script.
pub fn parse_key(name: &str) -> Result<Key> {
    let upper = name.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return Err(anyhow!("atalho incompleto: falta a tecla depois do '+'"));
    }
    if let Some((_, key)) = ALIASES.iter().find(|(alias, _)| *alias == upper) {
        return Ok(*key);
    }
    let bare = upper.strip_prefix("KEY_").unwrap_or(&upper);
    KEYS.iter()
        .find(|k| k.0 == bare)
        .map(|k| Key(k.1))
        .ok_or_else(|| anyhow!("tecla desconhecida: {name}"))
}

/// Lê um atalho como `SUPER+SHIFT+2`.
pub fn parse_combo(combo: &str) -> Result<Vec<Key>> {
    combo.split('+').map(parse_key).collect()
}

/// Nome da tecla para o script, preferindo o apelido.
pub fn key_name(key: Key) -> String {
    if let Some((alias, _)) = ALIASES.iter().find(|(_, k)| *k == key) {
        return (*alias).to_string();
    }
    KEYS.iter()
        .find(|k| k.1 == key.0)
        .map_or_else(|| format!("CODE{}", key.0), |k| k.0.to_string())
}

/// Atalho no formato do script.
pub fn combo_name(keys: &[Key]) -> String {
    keys.iter().map(|k| key_name(*k)).collect::<Vec<_>>().join("+")
}

/// Caractere produzido pela tecla, se for imprimível.
pub fn key_to_char(key: Key, shift: bool) -> Option<char> {
    US_LAYOUT
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, plain, shifted)| if shift { *shifted } else { *plain })
}

/// Tecla e shift necessários para digitar o caractere.
#[cfg(target_os = "linux")]
pub fn char_to_key(ch: char) -> Option<(Key, bool)> {
    match ch {
        '\n' => return Some((Key::ENTER, false)),
        '\t' => return Some((Key::TAB, false)),
        _ => {}
    }
    US_LAYOUT.iter().find_map(|(key, plain, shifted)| {
        if *plain == ch {
            Some((*key, false))
        } else if *shifted == ch {
            Some((*key, true))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_roundtrip_uses_aliases() {
        let keys = parse_combo("super+shift+2").unwrap();
        assert_eq!(keys, vec![Key::LEFTMETA, Key::LEFTSHIFT, Key(3)]);
        assert_eq!(combo_name(&keys), "SUPER+SHIFT+2");
        assert_eq!(parse_key("f10").unwrap(), Key::F10);
        assert_eq!(parse_key("KEY_ENTER").unwrap(), Key::ENTER);
        assert_eq!(key_name(Key::ENTER), "RETURN");
        assert!(parse_key("nope").is_err());
    }

    #[test]
    fn windows_vk_roundtrip() {
        for name in ["A", "2", "F10", "RETURN", "SUPER", "LEFT", "DEL", "KPENTER"] {
            let key = parse_key(name).unwrap();
            let vk = key.vk().unwrap();
            assert_eq!(Key::from_vk(vk).unwrap().vk(), Some(vk), "{name}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn chars_map_both_ways() {
        for ch in "Hello, World! ~/a_b@c.d".chars() {
            let (key, shift) = char_to_key(ch).unwrap();
            assert_eq!(key_to_char(key, shift), Some(ch));
        }
        assert!(char_to_key('ç').is_none());
    }
}
