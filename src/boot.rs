use crate::constants::{BOOT_MARKER, BOOT_SETTLE};
use crate::platform::Desktop;

/// Passos que esperam o desktop ficar pronto, conforme o ambiente detectado.
pub fn boot_prefix(desktop: Option<&Desktop>) -> Vec<String> {
    let mut steps = vec![BOOT_MARKER.to_string()];
    steps.extend(desktop.map(Desktop::boot_waits).unwrap_or_default());
    steps.push(format!("sleep {BOOT_SETTLE}"));
    steps
}

/// Põe o prefixo no início, se ainda não estiver lá.
pub fn ensure_prefix(steps: &mut Vec<String>, desktop: Option<&Desktop>) -> bool {
    if steps.iter().any(|s| s.trim() == BOOT_MARKER) {
        return false;
    }
    steps.splice(0..0, boot_prefix(desktop));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_goes_first_once() {
        let mut steps = vec!["launch kitty".to_string()];
        assert!(ensure_prefix(&mut steps, None));
        assert_eq!(steps[0], BOOT_MARKER);
        assert_eq!(steps.last().unwrap(), "launch kitty");
        let len = steps.len();
        assert!(!ensure_prefix(&mut steps, None));
        assert_eq!(steps.len(), len);
    }
}
