//! Pixel art feita no Aseprite (`assets/sprites/*.aseprite`), exportada como
//! faixa horizontal PNG + JSON (Array) com a duração de cada quadro.
//! Depois de editar, rode `assets/export.sh`.

use std::io::Cursor;
use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

type Rgb = (u8, u8, u8);

/// Desenhos disponíveis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sprite {
    Mascot,
    MascotSleep,
    MascotCheer,
    Rec,
    Paused,
    Running,
    Ok,
    Fail,
    Event,
    Clock,
    Interval,
}

/// Quantidade de sprites, para o cache.
const COUNT: usize = Sprite::Interval as usize + 1;
/// Instante que mostra o último quadro de um sprite que não repete.
pub const HOLD: u64 = u64::MAX;

impl Sprite {
    /// Sprites de resultado tocam uma vez e param no último quadro.
    fn looping(self) -> bool {
        !matches!(self, Sprite::Ok | Sprite::Fail)
    }
}

struct Frame {
    /// Pixels por linha; `None` é transparente.
    rows: Vec<Vec<Option<Rgb>>>,
    ms: u64,
}

struct Anim {
    frames: Vec<Frame>,
    total_ms: u64,
}

macro_rules! asset {
    ($n:literal) => {
        (
            include_bytes!(concat!("../../assets/sprites/", $n, ".png")).as_slice(),
            include_bytes!(concat!("../../assets/sprites/", $n, ".json")).as_slice(),
        )
    };
}

fn asset(sprite: Sprite) -> (&'static [u8], &'static [u8]) {
    match sprite {
        Sprite::Mascot => asset!("mascot"),
        Sprite::MascotSleep => asset!("mascot_sleep"),
        Sprite::MascotCheer => asset!("mascot_cheer"),
        Sprite::Rec => asset!("rec"),
        Sprite::Paused => asset!("paused"),
        Sprite::Running => asset!("running"),
        Sprite::Ok => asset!("ok"),
        Sprite::Fail => asset!("fail"),
        Sprite::Event => asset!("event"),
        Sprite::Clock => asset!("clock"),
        Sprite::Interval => asset!("interval"),
    }
}

fn decode(png: &[u8], json: &[u8]) -> Anim {
    let mut reader = png::Decoder::new(Cursor::new(png)).read_info().expect("sprite png");
    let mut buf = vec![0; reader.output_buffer_size().expect("tamanho do png")];
    let info = reader.next_frame(&mut buf).expect("dados do png");
    assert!(
        info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight,
        "exporte os sprites em RGBA 8 bits"
    );
    let meta: serde_json::Value = serde_json::from_slice(json).expect("sprite json");
    let frames: Vec<Frame> = meta["frames"]
        .as_array()
        .expect("frames no json")
        .iter()
        .map(|f| {
            let n = |k: &str| f["frame"][k].as_u64().expect("retângulo do quadro") as usize;
            let (x0, y0, w, h) = (n("x"), n("y"), n("w"), n("h"));
            let rows = (y0..y0 + h)
                .map(|y| {
                    (x0..x0 + w)
                        .map(|x| {
                            let p = &buf[(y * info.width as usize + x) * 4..][..4];
                            (p[3] > 127).then_some((p[0], p[1], p[2]))
                        })
                        .collect()
                })
                .collect();
            Frame {
                rows,
                ms: f["duration"].as_u64().unwrap_or(100).max(1),
            }
        })
        .collect();
    let total_ms = frames.iter().map(|f| f.ms).sum();
    Anim { frames, total_ms }
}

fn anim(sprite: Sprite) -> &'static Anim {
    static CACHE: [OnceLock<Anim>; COUNT] = [const { OnceLock::new() }; COUNT];
    CACHE[sprite as usize].get_or_init(|| {
        let (png, json) = asset(sprite);
        decode(png, json)
    })
}

fn frame_at(sprite: Sprite, t_ms: u64) -> &'static Frame {
    let anim = anim(sprite);
    let mut t = if sprite.looping() { t_ms % anim.total_ms } else { t_ms.min(anim.total_ms - 1) };
    for frame in &anim.frames {
        if t < frame.ms {
            return frame;
        }
        t -= frame.ms;
    }
    &anim.frames[0]
}

/// Tamanho em células: 1 coluna por pixel, 2 pixels por linha.
pub fn size(sprite: Sprite) -> (u16, u16) {
    let frame = &anim(sprite).frames[0];
    let width = frame.rows.first().map_or(0, Vec::len) as u16;
    (width, (frame.rows.len() as u16).div_ceil(2))
}

/// Desenha o quadro do instante `t_ms` com meio-blocos, recortado em `area`.
pub fn draw(buf: &mut Buffer, area: Rect, sprite: Sprite, t_ms: u64) {
    let frame = frame_at(sprite, t_ms);
    let pixel = |x: usize, y: usize| frame.rows.get(y).and_then(|row| row.get(x)).copied().flatten();
    let width = frame.rows.first().map_or(0, Vec::len);
    for row in 0..frame.rows.len().div_ceil(2) {
        for col in 0..width {
            let (x, y) = (area.x + col as u16, area.y + row as u16);
            if x >= area.right() || y >= area.bottom() {
                continue;
            }
            let rgb = |p: Rgb| Color::Rgb(p.0, p.1, p.2);
            let cell = &mut buf[(x, y)];
            match (pixel(col, row * 2), pixel(col, row * 2 + 1)) {
                (None, None) => {}
                (Some(top), None) => {
                    cell.set_char('▀').set_fg(rgb(top));
                }
                (None, Some(bottom)) => {
                    cell.set_char('▄').set_fg(rgb(bottom));
                }
                (Some(top), Some(bottom)) => {
                    cell.set_char('▀').set_fg(rgb(top)).set_bg(rgb(bottom));
                }
            }
        }
    }
}
