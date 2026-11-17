use anyhow::{Context, Result, anyhow, bail};
use jiff::Zoned;
use jiff::civil::Time;
use serde::{Deserialize, Serialize};

use crate::config::Trigger;

/// Dia da semana no formato do arquivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// Dias com o código do arquivo e o rótulo em pt-BR.
const WEEKDAYS: &[(Weekday, &str, &str)] = &[
    (Weekday::Mon, "mon", "seg"),
    (Weekday::Tue, "tue", "ter"),
    (Weekday::Wed, "wed", "qua"),
    (Weekday::Thu, "thu", "qui"),
    (Weekday::Fri, "fri", "sex"),
    (Weekday::Sat, "sat", "sáb"),
    (Weekday::Sun, "sun", "dom"),
];

impl Weekday {
    /// Rótulo curto em pt-BR.
    pub fn label(self) -> &'static str {
        WEEKDAYS.iter().find(|(d, _, _)| *d == self).map(|(_, _, l)| *l).unwrap_or("?")
    }

    /// Aceita `seg`, `sab`, `mon` etc.
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim().to_lowercase().replace('á', "a");
        WEEKDAYS
            .iter()
            .find(|(_, code, label)| *code == raw || label.replace('á', "a") == raw)
            .map(|(d, _, _)| *d)
            .ok_or_else(|| anyhow!("dia desconhecido: {raw} (use seg, ter, qua, qui, sex, sab, dom)"))
    }

    /// Mesma ordem segunda→domingo do jiff.
    fn matches(self, day: jiff::civil::Weekday) -> bool {
        self as i8 == day.to_monday_zero_offset()
    }
}

/// Lê `HH:MM`.
pub fn parse_clock(raw: &str) -> Result<Time> {
    let (h, m) = raw
        .trim()
        .split_once(':')
        .ok_or_else(|| anyhow!("horário deve ser HH:MM: {raw}"))?;
    let hour: i8 = h.parse().with_context(|| format!("hora inválida: {raw}"))?;
    let minute: i8 = m.parse().with_context(|| format!("minuto inválido: {raw}"))?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        bail!("horário fora do intervalo: {raw}");
    }
    Ok(Time::constant(0, 0, 0, 0).with().hour(hour).minute(minute).build()?)
}

/// Próximo disparo de um gatilho diário estritamente depois de `after`.
pub fn next_daily(at: &str, days: &[Weekday], after: &Zoned) -> Result<Zoned> {
    let time = parse_clock(at)?;
    for offset in 0..=7 {
        let date = after.date().checked_add(jiff::Span::new().days(offset))?;
        let candidate = date.to_datetime(time).to_zoned(after.time_zone().clone())?;
        let day_ok = days.is_empty() || days.iter().any(|d| d.matches(date.weekday()));
        if day_ok && candidate > *after {
            return Ok(candidate);
        }
    }
    bail!("sem próximo disparo para {at}")
}

/// Texto curto do próximo disparo, para a interface.
pub fn describe_next(trigger: &Trigger, now: &Zoned) -> Option<String> {
    match trigger {
        Trigger::Daily { at, days } => next_daily(at, days, now).ok().map(|next| {
            let day = WEEKDAYS[next.weekday().to_monday_zero_offset() as usize].2;
            format!("{day} {}", next.strftime("%d/%m %H:%M"))
        }),
        _ => None,
    }
}

/// Lê o texto da interface `09:00 seg,ter` num gatilho diário.
pub fn parse_daily_input(raw: &str) -> Result<Trigger> {
    let mut parts = raw.split_whitespace();
    let at = parts.next().ok_or_else(|| anyhow!("informe o horário HH:MM"))?;
    parse_clock(at)?;
    let days = parts
        .flat_map(|p| p.split(','))
        .filter(|p| !p.is_empty())
        .map(Weekday::parse)
        .collect::<Result<Vec<_>>>()?;
    Ok(Trigger::Daily {
        at: at.to_string(),
        days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(raw: &str) -> Zoned {
        raw.parse().unwrap()
    }

    #[test]
    fn next_daily_respects_time_and_days() {
        let monday_10 = at("2026-11-16T10:00:00-03:00[America/Sao_Paulo]");
        let next = next_daily("09:00", &[], &monday_10).unwrap();
        assert_eq!(next.date().to_string(), "2026-11-17");
        let next = next_daily("11:30", &[], &monday_10).unwrap();
        assert_eq!(next.date().to_string(), "2026-11-16");
        let next = next_daily("09:00", &[Weekday::Fri], &monday_10).unwrap();
        assert_eq!(next.date().to_string(), "2026-11-20");
    }

    #[test]
    fn parses_daily_input() {
        assert_eq!(
            parse_daily_input("08:30 seg,sex").unwrap(),
            Trigger::Daily {
                at: "08:30".into(),
                days: vec![Weekday::Mon, Weekday::Fri]
            }
        );
        assert!(parse_daily_input("25:00").is_err());
        assert!(parse_daily_input("08:00 xyz").is_err());
    }
}
