//! The ASCII sky over Today: the sun on its real sunrise→sunset arc, clouds and rain from the
//! forecast, stars at night and one of eight landscapes (smooth silhouettes with the ASCII riding
//! on them), lights going off as the day starts. Colours come from the CSS tokens.
//! prefers-reduced-motion: one still frame. Off screen or hidden: no drawing.
use crate::pages::mail::lib::{rgb, token_rgb};
use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

pub const SCENES: [&str; 8] = ["mountains", "coast", "hills", "lake", "dunes", "meadow", "forest", "city"];
pub fn scene_label(s: &str) -> &'static str {
    match s {
        "mountains" => "Mountains",
        "coast" => "Coast",
        "hills" => "Hills",
        "lake" => "Lake",
        "dunes" => "Dunes",
        "meadow" => "Flower meadow",
        "forest" => "Forest",
        _ => "City",
    }
}
const LAST_KEY: &str = "cerq-scene-last";
fn rnd() -> f64 {
    js_sys::Math::random()
}
/// A random landscape, never the one shown last time.
pub fn pick_scene(avoid: Option<&str>) -> &'static str {
    let pool: Vec<&str> = SCENES.iter().copied().filter(|x| Some(*x) != avoid).collect();
    pool[(rnd() * pool.len() as f64) as usize % pool.len()]
}
pub fn first_scene() -> &'static str {
    if let Some(q) = agentum_ui::nav::read().get("scene").and_then(|q| SCENES.iter().find(|s| *s == q)) {
        return q;
    }
    pick_scene(agentum_ui::store::get(LAST_KEY).as_deref())
}
pub fn remember_scene(s: &str) {
    agentum_ui::store::set(LAST_KEY, s);
}

#[derive(Clone, Copy, PartialEq)]
pub struct SkyState {
    pub cloud: f64,
    pub rain: f64,
    pub drizzle: bool,
    pub storm: bool,
    pub sunrise: f64,
    pub sunset: f64,
}
impl Default for SkyState {
    fn default() -> Self {
        SkyState { cloud: 0.2, rain: 0.0, drizzle: false, storm: false, sunrise: 360.0, sunset: 1170.0 }
    }
}
/// The sky from the brief's weather words ("Partly cloudy", "Light rain"…).
pub fn sky_from(w: &serde_json::Value) -> SkyState {
    let now = w["sky"].as_str().unwrap_or("").to_lowercase();
    let s = format!("{now} {}", w["day_sky"].as_str().unwrap_or("").to_lowercase());
    let any = |t: &str, ws: &[&str]| ws.iter().any(|x| t.contains(x));
    let wet = ["rain", "drizzle", "shower", "thunder", "lluvia", "llovizna", "chaparr", "tormenta"];
    let m = |k: &str| w[k].as_str().filter(|x| !x.is_empty()).map(|x| super::plan::mins(x) as f64);
    SkyState {
        cloud: if any(&s, &["overcast", "fog", "nublado", "niebla"]) {
            0.75
        } else if any(&s, &wet) {
            0.6
        } else if any(&s, &["partly", "parcial"]) {
            0.38
        } else if any(&s, &["mostly clear", "mayormente"]) {
            0.16
        } else {
            0.04
        },
        drizzle: any(&now, &["drizzle", "llovizna"]),
        storm: any(&now, &["thunder", "tormenta"]),
        rain: if any(&now, &wet) { 1.0 } else { 0.0 },
        sunrise: m("sunrise").filter(|x| *x > 0.0).unwrap_or(360.0),
        sunset: m("sunset").filter(|x| *x > 0.0).unwrap_or(1170.0),
    }
}

type Rgb = [f64; 3];
fn hex_or_rgb(v: &str) -> Rgb {
    let v = v.trim();
    if let Some(h) = v.strip_prefix('#').filter(|h| h.len() == 6) {
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(128) as f64;
        return [p(0), p(2), p(4)];
    }
    if v.starts_with("rgb") { rgb(v) } else { [128.0, 128.0, 160.0] }
}
fn css(c: Rgb) -> String {
    format!("rgb({},{},{})", c[0].round(), c[1].round(), c[2].round())
}
fn mix(a: &str, b: &str, k: f64) -> String {
    let (x, y) = (hex_or_rgb(a), hex_or_rgb(b));
    css([x[0] * k + y[0] * (1.0 - k), x[1] * k + y[1] * (1.0 - k), x[2] * k + y[2] * (1.0 - k)])
}
/// Whether a colour is a blue (hue 205°–255°, not grey): then it can be the sky as it is.
fn is_blue(c: &str) -> bool {
    let [r, g, b] = hex_or_rgb(c).map(|v| v / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d < 0.08 {
        return false;
    }
    let h = if max == r { ((g - b) / d) % 6.0 } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
    let deg = (h * 60.0 + 360.0) % 360.0;
    (205.0..=255.0).contains(&deg)
}
fn hash(x: f64, y: f64) -> f64 {
    let s = (x * 127.1 + y * 311.7).sin() * 43758.5453;
    s - s.floor()
}
fn noise(x: f64, y: f64) -> f64 {
    let (xi, yi) = (x.floor(), y.floor());
    let (xf, yf) = (x - xi, y - yi);
    let (u, v) = (xf * xf * (3.0 - 2.0 * xf), yf * yf * (3.0 - 2.0 * yf));
    let (a, b, c, d) = (hash(xi, yi), hash(xi + 1.0, yi), hash(xi, yi + 1.0), hash(xi + 1.0, yi + 1.0));
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}
fn fbm(x: f64, seed: f64) -> f64 {
    noise(x, seed) * 0.6 + noise(x * 2.1, seed + 7.0) * 0.28 + noise(x * 4.3, seed + 13.0) * 0.12
}
fn ridged(x: f64, seed: f64) -> f64 {
    1.0 - (2.0 * fbm(x, seed) - 1.0).abs()
}
/// 0 at night → 1 in daylight, from the sun's place (p: 0 sunrise, 1 sunset).
fn dayness(p: f64) -> f64 {
    (p.min(1.0 - p) * 8.0 + 0.4).clamp(0.0, 1.0)
}
pub fn warm_on(light: bool) -> &'static str {
    if light { "#d97a12" } else { "#f5c88a" }
}

#[derive(Default, Clone)]
struct Colours {
    bg: String,
    hl: String,
    base: String,
    sub: String,
    primary: String,
    positive: String,
    negative: String,
    warm: String,
    sky: String,
}
struct Poly {
    pts: Vec<(f64, f64)>,
    bottom: f64,
    color: String,
}
struct Water {
    y: f64,
    top: i64,
    refl: Option<Box<dyn Fn(f64, f64) -> bool>>,
    fill: String,
    wave: String,
    glint: String,
    mirror: String,
}
#[derive(Default)]
struct Land {
    cells: Vec<(i64, i64, char, String)>,
    polys: Vec<Poly>,
    lights: Vec<(i64, i64)>,
    water: Option<Water>,
}
struct Building {
    x: i64,
    w: i64,
    h: i64,
    seed: f64,
    roof: u8,
}

struct Engine {
    cv: web_sys::HtmlCanvasElement,
    hero: web_sys::HtmlElement,
    ctx: web_sys::CanvasRenderingContext2d,
    still: bool,
    scene: String,
    sky: SkyState,
    w: f64,
    h: f64,
    cw: f64,
    ch: f64,
    cols: i64,
    rows: i64,
    horizon: i64,
    narrow: bool,
    strip: bool,
    stars: Vec<(i64, i64, f64, f64)>,
    city: Vec<Building>,
    drops: Vec<(f64, f64, f64)>,
    land: Option<Land>,
    t0: f64,
    c: Colours,
    light: bool,
    tint: f64,
    cells: HashMap<i64, (char, String)>,
}

fn now_min() -> f64 {
    let d = js_sys::Date::new_0();
    (d.get_hours() * 60 + d.get_minutes()) as f64
}

impl Engine {
    fn colours(&mut self) {
        let v = token_rgb;
        self.c = Colours { bg: v("bg"), hl: v("hi"), base: v("base"), sub: v("subdue"), primary: v("primary"), positive: v("positive"), negative: v("negative"), ..Default::default() };
        let [r, g, b] = hex_or_rgb(&self.c.bg);
        self.light = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0 > 0.5;
        self.c.warm = warm_on(self.light).into();
        // The sky is blue: the theme's accent when it is a blue (Glance), else a sky blue.
        self.c.sky = if is_blue(&self.c.primary) { self.c.primary.clone() } else if self.light { "#3f7fe0".into() } else { "#6d97f0".into() };
        let p = hex_or_rgb(&self.c.sky);
        let chroma = (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) / 255.0;
        self.tint = if self.light { 1.0 } else { (0.36 / chroma.max(0.01)).min(1.0) };
    }
    fn sun_p(&self) -> f64 {
        (now_min() - self.sky.sunrise) / (self.sky.sunset - self.sky.sunrise).max(60.0)
    }
    fn layout(&mut self) {
        self.colours();
        let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0).clamp(1.0, 2.0);
        self.w = self.cv.client_width() as f64;
        self.h = self.cv.client_height() as f64;
        self.strip = self.h < 200.0;
        self.narrow = self.strip || self.w < 640.0;
        self.cv.set_width((self.w * dpr) as u32);
        self.cv.set_height((self.h * dpr) as u32);
        let _ = self.ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
        let fs: f64 = if self.strip { 11.0 } else if self.narrow { 12.0 } else { 14.0 };
        self.ctx.set_font(&format!("{fs}px 'JetBrains Mono', monospace"));
        self.cw = self.ctx.measure_text("M").map(|m| m.width()).ok().filter(|w| *w > 0.0).unwrap_or(8.4);
        self.ch = (fs * 1.18).round();
        self.cols = (self.w / self.cw).ceil() as i64;
        self.rows = (self.h / self.ch).ceil() as i64;
        self.horizon = self.rows - 2;
        self.stars = (0..((self.cols * self.rows) as f64 * 0.035) as usize)
            .map(|_| ((rnd() * self.cols as f64) as i64, (rnd() * (self.horizon - 3).max(1) as f64) as i64, rnd() * 6.28, rnd()))
            .collect();
        self.city.clear();
        if self.scene == "city" {
            let mut x = 0;
            while x < self.cols {
                let w = 3 + (rnd() * 6.0) as i64;
                let h = 2 + (rnd() * if self.strip { 3.0 } else { 9f64.min(self.rows as f64 * 0.32) }) as i64;
                if rnd() < 0.18 {
                    x += 1 + (rnd() * 3.0) as i64;
                    continue;
                }
                self.city.push(Building { x, w, h, seed: rnd() * 1000.0, roof: if rnd() < 0.25 { 0 } else if rnd() < 0.5 { 1 } else { 2 } });
                x += w + if rnd() < 0.5 { 0 } else { 1 };
            }
        }
        self.land = if self.scene == "city" { None } else { Some(self.make_land()) };
        self.drops.clear();
    }

    fn make_land(&self) -> Land {
        let c = &self.c;
        let (h, w, cw, chh, cols, light) = (self.h, self.w, self.cw, self.ch, self.cols, self.light);
        let kind = self.scene.as_str();
        let seed = (rnd() * 1000.0).floor();
        let vis = if self.strip { 0.96 } else { (1.0 - 70.0 / h).max(0.6) };
        let mut fill = if light { [mix(&c.sub, &c.bg, 0.14), mix(&c.sub, &c.bg, 0.26), mix(&c.sub, &c.bg, 0.4)] } else { [mix(&c.sub, &c.bg, 0.36), mix(&c.sub, &c.bg, 0.2), mix(&c.bg, "#000000", 0.8)] };
        let edge = if light { [mix(&c.sub, &c.bg, 0.4), mix(&c.sub, &c.bg, 0.58), mix(&c.base, &c.bg, 0.72)] } else { [mix(&c.base, &c.bg, 0.5), mix(&c.base, &c.bg, 0.4), mix(&c.base, &c.bg, 0.32)] };
        let tex = if light { [mix(&c.sub, &c.bg, 0.24), mix(&c.sub, &c.bg, 0.4), mix(&c.sub, &c.bg, 0.56)] } else { [mix(&c.base, &c.bg, 0.22), mix(&c.base, &c.bg, 0.17), mix(&c.sub, &c.bg, 0.42)] };
        let snow = mix(&c.hl, &c.bg, if light { 0.5 } else { 0.82 });
        let mut out = Land::default();
        if matches!(kind, "meadow" | "forest" | "hills") {
            for (k, f) in fill.iter_mut().enumerate() {
                *f = mix(&c.positive, f, if light { 0.14 + k as f64 * 0.05 } else { 0.07 + k as f64 * 0.03 });
            }
        }
        let bloom: Vec<String> = [c.warm.clone(), c.negative.clone(), c.hl.clone(), mix(&c.primary, &c.hl, 0.5), mix(&c.negative, &c.warm, 0.5)].into_iter().map(|x| if light { x } else { mix(&x, &c.bg, 0.82) }).collect();
        let grass = mix(&c.positive, &c.bg, if light { 0.6 } else { 0.45 });
        let water_y = match kind {
            "coast" => Some(h * (vis - 0.16)),
            "lake" => Some(h * (vis - 0.13)),
            _ => None,
        };
        let wy = water_y.unwrap_or(0.0);
        type F = Box<dyn Fn(f64) -> f64>;
        let layers: Vec<F> = match kind {
            "mountains" => vec![
                Box::new(move |x| h * (vis - 0.2 - 0.17 * ridged(x * 0.045, seed).powf(1.3))),
                Box::new(move |x| h * (vis - 0.11 - 0.13 * ridged(x * 0.07 + 40.0, seed + 9.0).powf(1.2))),
                Box::new(move |x| h * (vis - 0.03 - 0.07 * fbm(x * 0.11 + 80.0, seed + 21.0))),
            ],
            "lake" => vec![Box::new(move |x| wy - h * (0.1 + 0.2 * ridged(x * 0.05, seed).powf(1.3))), Box::new(move |x| wy - h * (0.03 + 0.09 * ridged(x * 0.08 + 40.0, seed + 9.0).powf(1.2)))],
            "hills" => vec![
                Box::new(move |x| h * (vis - 0.2 - 0.1 * fbm(x * 0.025, seed))),
                Box::new(move |x| h * (vis - 0.12 - 0.09 * fbm(x * 0.035 + 50.0, seed + 5.0))),
                Box::new(move |x| h * (vis - 0.04 - 0.06 * fbm(x * 0.05 + 90.0, seed + 11.0))),
            ],
            "dunes" => vec![
                Box::new(move |x| h * (vis - 0.15 - 0.08 * (0.5 + 0.5 * (x * 0.07 + seed).sin()) - 0.04 * fbm(x * 0.05, seed))),
                Box::new(move |x| h * (vis - 0.05 - 0.07 * (0.5 + 0.5 * (x * 0.11 + seed * 2.0).sin()) - 0.03 * fbm(x * 0.07, seed + 3.0))),
            ],
            "meadow" => vec![Box::new(move |x| h * (vis - 0.22 - 0.08 * fbm(x * 0.02, seed))), Box::new(move |x| h * (vis - 0.1 - 0.07 * fbm(x * 0.04 + 50.0, seed + 5.0)))],
            "forest" => vec![
                Box::new(move |x| h * (vis - 0.26 - 0.1 * fbm(x * 0.03, seed))),
                Box::new(move |x| h * (vis - 0.16 - 0.09 * fbm(x * 0.045 + 50.0, seed + 5.0))),
                Box::new(move |x| h * (vis - 0.06 - 0.06 * fbm(x * 0.06 + 90.0, seed + 11.0))),
            ],
            _ => {
                let left = hash(seed, 1.0) > 0.5;
                let colsf = cols as f64;
                let hc = colsf * if left { 0.18 } else { 0.78 };
                let hw = colsf * 0.34;
                let ic = colsf * if left { 0.7 } else { 0.3 };
                vec![
                    Box::new(move |x| {
                        let k = (1.0 - (x - ic).abs() / (colsf * 0.08)).max(0.0);
                        wy - if k > 0.0 { h * (0.05 * k.powf(0.7) + 0.02 * fbm(x * 0.2, seed + 3.0)) } else { -2.0 }
                    }),
                    Box::new(move |x| {
                        let k = (1.0 - (x - hc).abs() / hw).max(0.0);
                        wy - if k > 0.0 { h * (0.22 * k.powf(0.75) + 0.04 * fbm(x * 0.15, seed)) } else { -2.0 }
                    }),
                ]
            }
        };
        let floor_y = water_y.unwrap_or(h + chh);
        let n = layers.len();
        for (li, ridge) in layers.iter().enumerate() {
            let lf = li as f64;
            let depth = if n == 1 { 2 } else { ((li * 2) as f64 / (n - 1) as f64).round() as usize };
            let mut pts = vec![];
            let mut px = 0.0;
            while px <= w + 4.0 {
                pts.push((px, ridge(px / cw)));
                px += 4.0;
            }
            out.polys.push(Poly { pts, bottom: floor_y, color: fill[depth].clone() });
            let last_row = (floor_y / chh).ceil() as i64 - 1;
            for x in 0..cols {
                let xf = x as f64;
                let (y0, yl, yr) = (ridge(xf + 0.5), ridge(xf - 0.3), ridge(xf + 1.3));
                if y0 >= floor_y - 2.0 {
                    continue;
                }
                let r = (y0 / chh - 0.5).round() as i64;
                let d = yr - yl;
                let peak = y0 < yl - 1.0 && y0 < yr - 1.0;
                let flat = match kind {
                    "dunes" => '~',
                    "forest" => ' ',
                    _ => '-',
                };
                let ch = if peak && (kind == "mountains" || kind == "lake") { '^' } else if d < -chh * 0.3 { '/' } else if d > chh * 0.3 { '\\' } else if hash(xf, lf + seed) > 0.55 { flat } else { ' ' };
                let snowy = (kind == "mountains" || kind == "lake") && depth == 0 && y0 < h * (vis - 0.31);
                out.cells.push((x, r, ch, if snowy { snow.clone() } else { edge[depth].clone() }));
                for y in r + 1..=last_row {
                    let yf = y as f64;
                    let hh = hash(xf * 3.0 + lf, yf * 7.0 + seed);
                    let mut cc = if kind == "dunes" { if hh > 0.92 { '.' } else { ' ' } } else if hh > 0.95 { ':' } else if hh > 0.86 { '.' } else { ' ' };
                    let mut col = tex[depth].clone();
                    if snowy && y == r + 1 && hash(xf, seed) > 0.45 {
                        cc = '.';
                        col = snow.clone();
                    }
                    out.cells.push((x, y, cc, col));
                }
                if kind == "hills" && depth > 0 && fbm(xf * 0.12, seed + 30.0 + lf) > 0.58 && hash(xf, lf + 2.0) > 0.5 {
                    out.cells.push((x, r - 1, 'A', edge[depth].clone()));
                    if depth == 2 && hash(xf, 77.0) > 0.4 {
                        out.cells.push((x, r - 2, '^', edge[depth].clone()));
                    }
                }
                if kind == "dunes" && depth == 2 && hash(xf, seed + 5.0) > 0.94 {
                    out.cells.push((x, r - 1, '|', edge[2].clone()));
                    out.cells.push((x, r - 2, 'Y', edge[2].clone()));
                }
                if kind == "meadow" && depth == 2 {
                    for y in r..=last_row {
                        let yf = y as f64;
                        let hh = hash(xf * 5.0 + 1.0, yf * 3.0 + seed);
                        let dens = 0.45 - (y - r) as f64 * 0.06;
                        if hh < dens * 0.55 {
                            let chs = ['*', 'o', '@', '*', '.'];
                            out.cells.push((x, y, chs[(hash(xf, yf) * 5.0) as usize % 5], bloom[(hash(yf, xf + seed) * bloom.len() as f64) as usize % bloom.len()].clone()));
                        } else if hh < dens {
                            out.cells.push((x, y, if hash(xf + 3.0, yf) > 0.5 { ',' } else { '"' }, grass.clone()));
                        }
                    }
                    if hash(xf, seed + 2.0) > 0.8 {
                        out.cells.push((x, r - 1, if hash(xf, 9.0) > 0.5 { '*' } else { 'Y' }, bloom[(hash(xf, 5.0) * bloom.len() as f64) as usize % bloom.len()].clone()));
                    }
                }
                if kind == "meadow" && depth == 0 && hash(xf, seed + 4.0) > 0.9 {
                    out.cells.push((x, r - 1, '^', edge[0].clone()));
                }
                if kind == "forest" {
                    if hash(xf, lf + seed) > [0.72, 0.6, 0.5][depth] && fbm(xf * 0.2, seed + lf) > 0.35 {
                        out.cells.push((x, r - 1, if depth > 0 { 'A' } else { '^' }, edge[depth].clone()));
                        if depth >= 1 && hash(xf, 77.0 + lf) > 0.4 {
                            out.cells.push((x, r - 2, '^', edge[depth].clone()));
                        }
                        if depth == 2 && hash(xf, 91.0) > 0.55 {
                            out.cells.push((x, r - 2, 'A', edge[2].clone()));
                            out.cells.push((x, r - 3, '^', edge[2].clone()));
                        }
                    }
                    if depth == 2 && hash(xf, seed + 9.0) > 0.95 {
                        out.lights.push((x, last_row.min(r + 1)));
                    }
                }
                if (kind == "hills" || kind == "mountains") && depth == 2 && hash(xf, seed + 9.0) > 0.94 {
                    out.lights.push((x, last_row.min(r + 1 + (hash(xf, 4.0) * 2.0) as i64)));
                }
            }
        }
        if let Some(wy) = water_y {
            let refl: Option<Box<dyn Fn(f64, f64) -> bool>> = if kind == "lake" {
                let far = layers.into_iter().next();
                far.map(|f| Box::new(move |x: f64, y: f64| f(x + 0.5) < 2.0 * wy - (y + 0.5) * chh) as Box<dyn Fn(f64, f64) -> bool>)
            } else {
                None
            };
            out.water = Some(Water {
                y: wy,
                top: (wy / chh).floor() as i64,
                refl,
                fill: if light { mix(&c.sky, &c.bg, 0.22) } else { mix(&mix(&c.sky, &c.bg, 0.32 * self.tint), "#000000", 0.84) },
                wave: mix(&c.sky, &c.bg, if light { 0.55 } else { 0.42 * self.tint }),
                glint: c.warm.clone(),
                mirror: edge[0].clone(),
            });
        }
        out
    }

    fn gradient(&self) {
        let p = self.sun_p();
        let d = dayness(p);
        let edge = (1.0 - p.abs().min((1.0 - p).abs()) * 6.0).max(0.0);
        let k = self.tint;
        let c = &self.c;
        let top = mix(&c.sky, &c.bg, (0.08 + d * 0.22 * (1.0 - self.sky.cloud * 0.6)) * k);
        let low = mix(&c.warm, &mix(&c.sky, &c.bg, (0.12 + d * 0.2) * k), 0.05 + edge * 0.42 * (1.0 - self.sky.cloud * 0.5));
        let _ = self.hero.style().set_property("background", &format!("linear-gradient(180deg, {top} 0%, {} 55%, {low} 100%)", mix(&top, &low, 0.5)));
    }

    fn put(&mut self, x: i64, y: i64, ch: char, col: String) {
        if x >= 0 && y >= 0 && x < self.cols && y < self.rows {
            self.cells.insert(x + y * 4096, (ch, col));
        }
    }

    fn frame(&mut self, now: f64) {
        let t = (now - self.t0) / 1000.0;
        let p = self.sun_p();
        let d = dayness(p);
        self.cells.clear();
        let c = self.c.clone();
        let night = 1.0 - d;
        if night > 0.05 {
            for i in 0..self.stars.len() {
                let (x, y, ph, k) = self.stars[i];
                let tw = 0.5 + 0.5 * (t * (0.8 + k * 2.0) + ph).sin();
                if tw * night > 0.25 {
                    self.put(x, y, if k > 0.85 { '*' } else if k > 0.6 { '+' } else { '.' }, mix(&c.hl, &c.bg, 0.25 + 0.6 * tw * night));
                }
            }
        }
        // The sun: rises from the horizon to where it is now (intro), on its arc.
        let intro = if self.still { 1.0 } else { (t / 2.6).min(1.0) };
        let ease = 1.0 - (1.0 - intro).powi(3);
        let pc = p.clamp(-0.08, 1.08);
        let (cols, rows, horizon, cw, chh) = (self.cols as f64, self.rows as f64, self.horizon as f64, self.cw, self.ch);
        let sx = (if self.narrow { 0.52 + 0.43 * pc } else { 0.1 + 0.8 * pc }) * cols;
        let peak = (PI * pc.clamp(0.0, 1.0)).sin() * horizon * 0.78;
        let sy = horizon + 3.0 - (peak + 3.0) * ease;
        let rr = (rows * 0.13).clamp(2.6, 5.5) * chh;
        let spin = if self.still { 0.0 } else { t * 0.25 };
        let mut y = (sy - rr * 2.4 / chh).floor();
        while y <= sy + rr * 2.4 / chh {
            let mut x = (sx - rr * 2.4 / cw).floor();
            while x <= sx + rr * 2.4 / cw {
                let (dx, dy) = ((x - sx) * cw, (y - sy) * chh);
                let r = dx.hypot(dy) / rr;
                if y <= horizon {
                    if r < 1.0 {
                        let col = if self.light { mix(&c.warm, "#ffb000", 1.0 - r * 0.5) } else { mix("#fff6dc", &c.warm, 1.0 - r * 0.7) };
                        self.put(x as i64, y as i64, if r < 0.55 { '@' } else if r < 0.82 { '#' } else { '*' }, col);
                    } else if r < 1.25 {
                        self.put(x as i64, y as i64, if r < 1.1 { '+' } else { ':' }, mix(&c.warm, &c.bg, 0.75 - (r - 1.0) * 1.6));
                    } else if r < 2.3 {
                        let a = dy.atan2(dx);
                        let ray = (a * 9.0 + spin).cos();
                        if ray > 0.86 && hash(x, y + (t * 3.0).floor()) > 0.25 * r {
                            let deg = ((a * 180.0 / PI) + 360.0) % 180.0;
                            let ch = if !(22.0..=158.0).contains(&deg) { '-' } else if deg < 68.0 { '\\' } else if deg < 112.0 { '|' } else { '/' };
                            self.put(x as i64, y as i64, ch, mix(&c.warm, &c.bg, (0.62 - (r - 1.25) * 0.45).max(0.12)));
                        }
                    }
                }
                x += 1.0;
            }
            y += 1.0;
        }
        if self.sky.cloud > 0.02 {
            let th = 1.0 - self.sky.cloud * 0.62;
            let ramp = ['.', ':', '-', '~', '=', '('];
            for y in 0..(self.horizon - 1).max(0) {
                for x in 0..self.cols {
                    let (xf, yf) = (x as f64, y as f64);
                    let n = noise(xf * 0.07 + t * 0.12, yf * 0.22) * 0.65 + noise(xf * 0.16 - t * 0.05, yf * 0.45 + 9.0) * 0.35;
                    let v = (n * (1.0 - (yf / horizon) * 0.55) - th) / (1.0 - th);
                    if v > 0.0 {
                        let ch = ramp[((v * ramp.len() as f64 * 1.6) as usize).min(ramp.len() - 1)];
                        self.put(x, y, ch, mix(if d > 0.5 { &c.hl } else { &c.base }, &c.bg, 0.22 + v * 0.55 - self.sky.rain * 0.12));
                    }
                }
            }
        }
        if self.sky.rain > 0.0 {
            let want = (cols * if self.sky.drizzle { 0.45 } else { 0.8 } * self.sky.rain) as usize;
            while self.drops.len() < want {
                self.drops.push((rnd() * cols * 1.2, -rnd() * rows, (if self.sky.drizzle { 9.0 } else { 18.0 }) * (0.7 + rnd() * 0.6)));
            }
            let has_land = self.land.is_some();
            for i in 0..self.drops.len() {
                let (mut dx, mut dy, v) = self.drops[i];
                dy += v / 30.0;
                dx -= (v / 30.0) * 0.35;
                let (x, y) = (dx.round() as i64, dy.round() as i64);
                if y >= self.horizon || x < 0 {
                    if !has_land {
                        self.put(x, self.horizon, ',', mix(&c.base, &c.bg, 0.6));
                    }
                    dy = -rnd() * 6.0;
                    dx = rnd() * cols * 1.2;
                } else {
                    self.put(x, y, if self.sky.drizzle { '\'' } else { '/' }, mix(&c.sky, &c.bg, 0.8));
                }
                self.drops[i] = (dx, dy, v);
            }
        }
        if let Some(land) = self.land.take() {
            for (x, y, ch, col) in &land.cells {
                self.put(*x, *y, *ch, col.clone());
            }
            if night > 0.3 {
                for (x, y) in &land.lights {
                    if hash(*x as f64, (t / 7.0).floor()) > 0.15 {
                        self.put(*x, *y, '.', mix(&c.warm, &c.bg, 0.9));
                    }
                }
            }
            if let Some(w) = &land.water {
                let sun_up = sy * chh < w.y;
                for y in w.top..self.rows {
                    for x in 0..self.cols {
                        let (xf, yf) = (x as f64, y as f64);
                        let k = (xf * 0.55 + yf * 1.7 - t * 1.4 + noise(xf * 0.2, yf + t * 0.2) * 3.0).sin();
                        let near = (xf - sx).abs() < 1.2 + (y - w.top) as f64 * 0.8;
                        if sun_up && near && hash(xf, yf + (t * 6.0).floor()) > 0.35 {
                            self.put(x, y, if (x + y) % 2 != 0 { '=' } else { '-' }, mix(&w.glint, &c.bg, 0.85 - (y - w.top) as f64 * 0.07));
                        } else if w.refl.as_ref().is_some_and(|f| f(xf + (yf * 2.0 + t).sin() * 0.6, yf)) && hash(xf, yf) > 0.35 {
                            self.put(x, y, if k > 0.3 { '-' } else { '.' }, w.mirror.clone());
                        } else {
                            self.put(x, y, if k > 0.6 { '~' } else if k < -0.8 { '-' } else { ' ' }, w.wave.clone());
                        }
                    }
                }
            }
            self.land = Some(land);
        }
        let city = std::mem::take(&mut self.city);
        for b in &city {
            let top = self.rows - 1 - b.h;
            for y in top..self.rows {
                for x in b.x..b.x + b.w {
                    let edge = x == b.x || x == b.x + b.w - 1;
                    if y == top {
                        self.put(x, y, if b.roof == 0 && x == b.x + (b.w >> 1) { 'A' } else if edge { '.' } else { '_' }, mix(&c.base, &c.bg, 0.45));
                    } else if edge {
                        self.put(x, y, '|', mix(&c.base, &c.bg, 0.32));
                    } else {
                        let (xf, yf) = (x as f64, y as f64);
                        let lit = hash(xf + b.seed, yf) < 0.55 * night + 0.04 && hash(xf, yf + (t / 5.0 + b.seed).floor()) > 0.04;
                        self.put(x, y, if lit { ':' } else if hash(xf, yf) > 0.5 { '.' } else { ' ' }, if lit { mix(&c.warm, &c.bg, 0.9) } else { mix(&c.sub, &c.bg, 0.6) });
                    }
                }
            }
            if b.roof == 0 {
                self.put(b.x + (b.w >> 1), top - 1, '|', mix(&c.base, &c.bg, 0.45));
            }
        }
        self.city = city;
        if self.land.is_none() {
            for x in 0..self.cols {
                self.put(x, self.rows - 1, '_', mix(&c.base, &c.bg, 0.4));
            }
        }
        // Draw.
        let ctx = &self.ctx;
        ctx.clear_rect(0.0, 0.0, self.w, self.h);
        if self.sky.storm && hash((t * 4.0).floor(), 7.0) > 0.985 {
            ctx.set_fill_style_str(&mix(&c.hl, &c.bg, 0.1));
            ctx.fill_rect(0.0, 0.0, self.w, self.h);
        }
        ctx.set_fill_style_str(&if self.light { mix(&c.sub, &c.bg, 0.3) } else { mix(&c.bg, "#000000", 0.82) });
        for b in &self.city {
            ctx.fill_rect(b.x as f64 * cw, (self.rows - 1 - b.h) as f64 * chh, b.w as f64 * cw, (b.h + 1) as f64 * chh);
        }
        if let Some(land) = &self.land {
            for pl in &land.polys {
                ctx.set_fill_style_str(&pl.color);
                ctx.begin_path();
                ctx.move_to(0.0, pl.bottom);
                for (px, py) in &pl.pts {
                    ctx.line_to(*px, py.min(pl.bottom));
                }
                ctx.line_to(self.w + 4.0, pl.bottom);
                ctx.close_path();
                ctx.fill();
            }
            if let Some(w) = &land.water {
                ctx.set_fill_style_str(&w.fill);
                ctx.fill_rect(0.0, w.y, self.w, self.h);
            }
        }
        ctx.set_text_baseline("top");
        let mut buf = [0u8; 4];
        for (k, (ch, col)) in &self.cells {
            if *ch == ' ' {
                continue;
            }
            ctx.set_fill_style_str(col);
            let _ = ctx.fill_text(ch.encode_utf8(&mut buf), (k % 4096) as f64 * cw, (k / 4096) as f64 * chh);
        }
    }
}

/// A running sky: `set_sky`, `set_scene`, `restart` (new colours); dropping `stop`s it.
pub struct Sky {
    eng: Rc<RefCell<Engine>>,
    state: Rc<RefCell<Loop>>,
}
#[derive(Default)]
struct Loop {
    raf: i32,
    last: f64,
    dead: bool,
    on_screen: bool,
    tick: Option<Closure<dyn FnMut(f64)>>,
    keep: Vec<Box<dyn std::any::Any>>,
    ro: Option<web_sys::ResizeObserver>,
    io: Option<web_sys::IntersectionObserver>,
    vis: Option<Closure<dyn FnMut()>>,
}

fn perf_now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0)
}
fn hidden() -> bool {
    web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden())
}

impl Sky {
    pub fn start(cv: web_sys::HtmlCanvasElement, hero: web_sys::HtmlElement, scene: &str) -> Option<Sky> {
        let ctx: web_sys::CanvasRenderingContext2d = cv.get_context("2d").ok()??.dyn_into().ok()?;
        let still = web_sys::window().and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok().flatten()).is_some_and(|m| m.matches());
        let eng = Rc::new(RefCell::new(Engine {
            cv: cv.clone(),
            hero,
            ctx,
            still,
            scene: scene.into(),
            sky: SkyState::default(),
            w: 0.0,
            h: 0.0,
            cw: 8.4,
            ch: 16.0,
            cols: 0,
            rows: 0,
            horizon: 0,
            narrow: false,
            strip: false,
            stars: vec![],
            city: vec![],
            drops: vec![],
            land: None,
            t0: perf_now(),
            c: Colours::default(),
            light: false,
            tint: 1.0,
            cells: HashMap::new(),
        }));
        let state = Rc::new(RefCell::new(Loop { on_screen: true, ..Default::default() }));
        let sky = Sky { eng, state };
        // The frame loop: ~30 fps, ~16 on a strip.
        {
            let (eng, st) = (sky.eng.clone(), sky.state.clone());
            let tick = Closure::<dyn FnMut(f64)>::new(move |now: f64| {
                let mut s = st.borrow_mut();
                if s.dead || hidden() || !s.on_screen {
                    s.raf = 0;
                    return;
                }
                if let Some(t) = &s.tick {
                    s.raf = web_sys::window().and_then(|w| w.request_animation_frame(t.as_ref().unchecked_ref()).ok()).unwrap_or(0);
                }
                let strip = eng.borrow().strip;
                if now - s.last < if strip { 60.0 } else { 33.0 } {
                    return;
                }
                s.last = now;
                drop(s);
                eng.borrow_mut().frame(now);
            });
            sky.state.borrow_mut().tick = Some(tick);
        }
        // Resized: lay out again; off screen: stop drawing; back to the tab: wake.
        {
            let (eng, st) = (sky.eng.clone(), sky.state.clone());
            let timer: Rc<RefCell<Option<gloo_timers::callback::Timeout>>> = Rc::new(RefCell::new(None));
            let ro_cb = Closure::<dyn FnMut()>::new(move || {
                let (eng, st) = (eng.clone(), st.clone());
                *timer.borrow_mut() = Some(gloo_timers::callback::Timeout::new(120, move || Sky::restart_with(&eng, &st)));
            });
            if let Ok(ro) = web_sys::ResizeObserver::new(ro_cb.as_ref().unchecked_ref()) {
                ro.observe(&cv);
                sky.state.borrow_mut().ro = Some(ro);
            }
            sky.state.borrow_mut().keep.push(Box::new(ro_cb));
        }
        {
            let (eng, st) = (sky.eng.clone(), sky.state.clone());
            let io_cb = Closure::<dyn FnMut(js_sys::Array)>::new(move |e: js_sys::Array| {
                if let Ok(en) = e.get(0).dyn_into::<web_sys::IntersectionObserverEntry>() {
                    st.borrow_mut().on_screen = en.is_intersecting();
                    Sky::wake_with(&eng, &st);
                }
            });
            if let Ok(io) = web_sys::IntersectionObserver::new(io_cb.as_ref().unchecked_ref()) {
                io.observe(&cv);
                sky.state.borrow_mut().io = Some(io);
            }
            sky.state.borrow_mut().keep.push(Box::new(io_cb));
        }
        {
            let (eng, st) = (sky.eng.clone(), sky.state.clone());
            let vis = Closure::<dyn FnMut()>::new(move || Sky::wake_with(&eng, &st));
            if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                let _ = d.add_event_listener_with_callback("visibilitychange", vis.as_ref().unchecked_ref());
            }
            sky.state.borrow_mut().vis = Some(vis);
        }
        {
            let eng = sky.eng.clone();
            let minute = gloo_timers::callback::Interval::new(60_000, move || eng.borrow().gradient());
            sky.state.borrow_mut().keep.push(Box::new(minute));
        }
        Sky::restart_with(&sky.eng, &sky.state);
        Some(sky)
    }
    fn wake_with(eng: &Rc<RefCell<Engine>>, st: &Rc<RefCell<Loop>>) {
        let mut s = st.borrow_mut();
        if s.raf != 0 {
            if let Some(w) = web_sys::window() {
                let _ = w.cancel_animation_frame(s.raf);
            }
            s.raf = 0;
        }
        if s.dead {
            return;
        }
        if eng.borrow().still {
            let t = eng.borrow().t0 + 5000.0;
            drop(s);
            eng.borrow_mut().frame(t);
            return;
        }
        if !hidden() && s.on_screen {
            if let Some(t) = &s.tick {
                s.raf = web_sys::window().and_then(|w| w.request_animation_frame(t.as_ref().unchecked_ref()).ok()).unwrap_or(0);
            }
        }
    }
    fn restart_with(eng: &Rc<RefCell<Engine>>, st: &Rc<RefCell<Loop>>) {
        if st.borrow().dead {
            return;
        }
        {
            let mut e = eng.borrow_mut();
            e.layout();
            e.gradient();
        }
        Sky::wake_with(eng, st);
    }
    /// The same scene in new colours (a new look, light or dark).
    pub fn restart(&self) {
        Sky::restart_with(&self.eng, &self.state);
    }
    pub fn set_sky(&self, s: SkyState) {
        let mut e = self.eng.borrow_mut();
        if s.rain != e.sky.rain {
            e.drops.clear();
        }
        e.sky = s;
        e.gradient();
        if e.still {
            let t = e.t0 + 5000.0;
            e.frame(t);
        }
    }
    pub fn set_scene(&self, s: &str) {
        self.eng.borrow_mut().scene = s.into();
        self.restart();
    }
}
impl Drop for Sky {
    fn drop(&mut self) {
        let mut s = self.state.borrow_mut();
        s.dead = true;
        if let Some(w) = web_sys::window() {
            let _ = w.cancel_animation_frame(s.raf);
        }
        if let Some(ro) = &s.ro {
            ro.disconnect();
        }
        if let Some(io) = &s.io {
            io.disconnect();
        }
        if let (Some(v), Some(d)) = (s.vis.take(), web_sys::window().and_then(|w| w.document())) {
            let _ = d.remove_event_listener_with_callback("visibilitychange", v.as_ref().unchecked_ref());
        }
        s.keep.clear();
        s.tick = None;
    }
}
