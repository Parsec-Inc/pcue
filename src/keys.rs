use std::str::FromStr;

use anyhow::{Result, anyhow};
use evdev::Key;

/// Apelidos aceitos no script para modificadores e teclas comuns.
const ALIASES: &[(&str, Key)] = &[
    ("SUPER", Key::KEY_LEFTMETA),
    ("META", Key::KEY_LEFTMETA),
    ("WIN", Key::KEY_LEFTMETA),
    ("CTRL", Key::KEY_LEFTCTRL),
    ("CONTROL", Key::KEY_LEFTCTRL),
    ("ALT", Key::KEY_LEFTALT),
    ("ALTGR", Key::KEY_RIGHTALT),
    ("SHIFT", Key::KEY_LEFTSHIFT),
    ("RSUPER", Key::KEY_RIGHTMETA),
    ("RCTRL", Key::KEY_RIGHTCTRL),
    ("RSHIFT", Key::KEY_RIGHTSHIFT),
    ("RETURN", Key::KEY_ENTER),
    ("ESCAPE", Key::KEY_ESC),
    ("DEL", Key::KEY_DELETE),
];

/// Teclas sem shift e com shift no layout US.
const US_LAYOUT: &[(Key, char, char)] = &[
    (Key::KEY_1, '1', '!'),
    (Key::KEY_2, '2', '@'),
    (Key::KEY_3, '3', '#'),
    (Key::KEY_4, '4', '$'),
    (Key::KEY_5, '5', '%'),
    (Key::KEY_6, '6', '^'),
    (Key::KEY_7, '7', '&'),
    (Key::KEY_8, '8', '*'),
    (Key::KEY_9, '9', '('),
    (Key::KEY_0, '0', ')'),
    (Key::KEY_MINUS, '-', '_'),
    (Key::KEY_EQUAL, '=', '+'),
    (Key::KEY_LEFTBRACE, '[', '{'),
    (Key::KEY_RIGHTBRACE, ']', '}'),
    (Key::KEY_BACKSLASH, '\\', '|'),
    (Key::KEY_SEMICOLON, ';', ':'),
    (Key::KEY_APOSTROPHE, '\'', '"'),
    (Key::KEY_GRAVE, '`', '~'),
    (Key::KEY_COMMA, ',', '<'),
    (Key::KEY_DOT, '.', '>'),
    (Key::KEY_SLASH, '/', '?'),
    (Key::KEY_SPACE, ' ', ' '),
    (Key::KEY_Q, 'q', 'Q'),
    (Key::KEY_W, 'w', 'W'),
    (Key::KEY_E, 'e', 'E'),
    (Key::KEY_R, 'r', 'R'),
    (Key::KEY_T, 't', 'T'),
    (Key::KEY_Y, 'y', 'Y'),
    (Key::KEY_U, 'u', 'U'),
    (Key::KEY_I, 'i', 'I'),
    (Key::KEY_O, 'o', 'O'),
    (Key::KEY_P, 'p', 'P'),
    (Key::KEY_A, 'a', 'A'),
    (Key::KEY_S, 's', 'S'),
    (Key::KEY_D, 'd', 'D'),
    (Key::KEY_F, 'f', 'F'),
    (Key::KEY_G, 'g', 'G'),
    (Key::KEY_H, 'h', 'H'),
    (Key::KEY_J, 'j', 'J'),
    (Key::KEY_K, 'k', 'K'),
    (Key::KEY_L, 'l', 'L'),
    (Key::KEY_Z, 'z', 'Z'),
    (Key::KEY_X, 'x', 'X'),
    (Key::KEY_C, 'c', 'C'),
    (Key::KEY_V, 'v', 'V'),
    (Key::KEY_B, 'b', 'B'),
    (Key::KEY_N, 'n', 'N'),
    (Key::KEY_M, 'm', 'M'),
];

/// Modificadores reconhecidos na gravação.
pub const MODIFIERS: &[Key] = &[
    Key::KEY_LEFTCTRL,
    Key::KEY_RIGHTCTRL,
    Key::KEY_LEFTALT,
    Key::KEY_RIGHTALT,
    Key::KEY_LEFTMETA,
    Key::KEY_RIGHTMETA,
    Key::KEY_LEFTSHIFT,
    Key::KEY_RIGHTSHIFT,
];

/// A tecla é modificador?
pub fn is_modifier(key: Key) -> bool {
    MODIFIERS.contains(&key)
}

/// A tecla é shift?
pub fn is_shift(key: Key) -> bool {
    key == Key::KEY_LEFTSHIFT || key == Key::KEY_RIGHTSHIFT
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
    let full = if upper.starts_with("KEY_") || upper.starts_with("BTN_") {
        upper
    } else {
        format!("KEY_{upper}")
    };
    Key::from_str(&full).map_err(|_| anyhow!("tecla desconhecida: {name}"))
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
    let debug = format!("{key:?}");
    debug.strip_prefix("KEY_").unwrap_or(&debug).to_string()
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
pub fn char_to_key(ch: char) -> Option<(Key, bool)> {
    match ch {
        '\n' => return Some((Key::KEY_ENTER, false)),
        '\t' => return Some((Key::KEY_TAB, false)),
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
        assert_eq!(keys, vec![Key::KEY_LEFTMETA, Key::KEY_LEFTSHIFT, Key::KEY_2]);
        assert_eq!(combo_name(&keys), "SUPER+SHIFT+2");
        assert_eq!(parse_key("f10").unwrap(), Key::KEY_F10);
        assert!(parse_key("nope").is_err());
    }

    #[test]
    fn chars_map_both_ways() {
        for ch in "Hello, World! ~/a_b@c.d".chars() {
            let (key, shift) = char_to_key(ch).unwrap();
            assert_eq!(key_to_char(key, shift), Some(ch));
        }
        assert!(char_to_key('ç').is_none());
    }
}
