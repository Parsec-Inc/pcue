use crate::constants::{BOOT_MARKER, BOOT_SETTLE, BOOT_WAIT_TIMEOUT, KNOWN_BARS};
use crate::hypr::Hypr;
use crate::script::quote;

/// Passos que esperam o desktop ficar pronto, conforme o ambiente detectado.
pub fn boot_prefix(hypr: Option<&Hypr>) -> Vec<String> {
    let Some(hypr) = hypr else {
        return vec![BOOT_MARKER.to_string(), format!("sleep {BOOT_SETTLE}")];
    };
    let layers = hypr.layers().unwrap_or_default();
    let bar = KNOWN_BARS.iter().find(|bar| layers.iter().any(|l| l == *bar));
    let mut steps = vec![BOOT_MARKER.to_string()];
    if let Some(bar) = bar {
        steps.push(format!("wait_layer namespace={} timeout={BOOT_WAIT_TIMEOUT}", quote(bar)));
    }
    steps.push(format!("wait_monitors timeout={BOOT_WAIT_TIMEOUT}"));
    steps.push(format!("sleep {BOOT_SETTLE}"));
    steps
}

/// Põe o prefixo no início, se ainda não estiver lá.
pub fn ensure_prefix(steps: &mut Vec<String>, hypr: Option<&Hypr>) -> bool {
    if steps.iter().any(|s| s.trim() == BOOT_MARKER) {
        return false;
    }
    steps.splice(0..0, boot_prefix(hypr));
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
