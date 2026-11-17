use std::time::{Duration, Instant};

use ratatui::style::Color;

/// Duração das transições de seleção.
pub const SLIDE: Duration = Duration::from_millis(140);
/// Duração da abertura de janelas sobrepostas.
pub const POP: Duration = Duration::from_millis(180);
/// Quanto dura o brilho de "salvo".
pub const FLASH: Duration = Duration::from_millis(900);

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPINNER_FRAME_MS: u128 = 80;

/// Progresso 0..=1 desde `start`, com desaceleração no fim.
pub fn ease_out(start: Instant, duration: Duration) -> f32 {
    let t = (start.elapsed().as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Quadro atual do spinner.
pub fn spinner(clock: Instant) -> char {
    SPINNER[(clock.elapsed().as_millis() / SPINNER_FRAME_MS) as usize % SPINNER.len()]
}

/// Onda 0..=1 com o período dado.
pub fn wave(clock: Instant, period: Duration) -> f32 {
    let phase = clock.elapsed().as_secs_f32() / period.as_secs_f32() * std::f32::consts::TAU;
    (phase.sin() + 1.0) / 2.0
}

/// Mistura duas cores RGB.
pub fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color::Rgb(lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

/// Seleção que desliza da posição anterior até a atual.
#[derive(Debug, Clone, Copy)]
pub struct Slide {
    from: usize,
    to: usize,
    at: Instant,
}

impl Default for Slide {
    fn default() -> Self {
        Self {
            from: 0,
            to: 0,
            at: Instant::now().checked_sub(SLIDE).unwrap_or_else(Instant::now),
        }
    }
}

impl Slide {
    /// Registra a nova posição, partindo de onde a animação estava.
    pub fn follow(&mut self, to: usize) {
        if to != self.to {
            self.from = self.position().round() as usize;
            self.to = to;
            self.at = Instant::now();
        }
    }

    /// Posição animada (em linhas).
    pub fn position(&self) -> f32 {
        self.from as f32 + (self.to as f32 - self.from as f32) * self.progress()
    }

    /// Fração já percorrida, para o preenchimento da barra.
    pub fn progress(&self) -> f32 {
        ease_out(self.at, SLIDE)
    }

    pub fn moving(&self) -> bool {
        self.at.elapsed() < SLIDE
    }
}

/// Texto curto de duração: `2h 05m`, `4m 09s`, `12s`.
pub fn countdown(seconds: i64) -> String {
    let s = seconds.max(0);
    let (d, h, m, sec) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    match (d, h, m) {
        (d, h, _) if d > 0 => format!("{d}d {h:02}h"),
        (_, h, m) if h > 0 => format!("{h}h {m:02}m {sec:02}s"),
        (_, _, m) if m > 0 => format!("{m}m {sec:02}s"),
        _ => format!("{sec}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_formats() {
        assert_eq!(countdown(5), "5s");
        assert_eq!(countdown(65), "1m 05s");
        assert_eq!(countdown(3725), "1h 02m 05s");
        assert_eq!(countdown(90_000), "1d 01h");
    }
}
