//! The price chart a market row opens, full screen (CERQ's cerq-chart.js, drawn as SVG): ranges
//! 1D to MAX, area or candles, linear or log, a crosshair with that day's price, and how much the
//! price moved over the range. Prices from the hub's GET /api/chart (Yahoo Finance).
use super::widgets::price_text;
use agentum_ui::prelude::*;
use std::collections::HashMap;

/// What the row showed: the chart starts from it while its history loads.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub symbol: String,
    pub name: String,
    pub price: String,
    pub change: f64,
    pub currency: String,
}

thread_local! {
    static OPEN: RwSignal<Option<Item>> = agentum_ui::global(None);
}
pub fn open(it: Item) {
    OPEN.with(|o| o.set(Some(it)));
}
fn close() {
    OPEN.with(|o| o.set(None));
}

const RANGES: [&str; 7] = ["1D", "5D", "1M", "6M", "1Y", "5Y", "MAX"];
/// The hub's range that holds a button's points (1M and 6M are cut from the year).
fn source(r: &str) -> &'static str {
    match r {
        "1D" => "1d",
        "5D" => "5d",
        "5Y" => "5y",
        "MAX" => "max",
        _ => "1y",
    }
}
/// The first day a range shows, counted back from the last point's day ("YYYY-MM-DD"); None: all.
fn from_day(r: &str, last: &str) -> Option<String> {
    let months: i32 = match r {
        "1M" => 1,
        "6M" => 6,
        _ => return None,
    };
    let (mut y, mut m): (i32, i32) = (last.get(0..4)?.parse().ok()?, last.get(5..7)?.parse().ok()?);
    m -= months;
    while m < 1 {
        m += 12;
        y -= 1;
    }
    Some(format!("{y:04}-{m:02}-{}", last.get(8..10)?))
}
/// A coordinate for an SVG attribute.
fn f(v: f64) -> String {
    format!("{v:.2}")
}

#[derive(Clone, Debug, PartialEq)]
struct Pt {
    time: String,
    v: f64,
    o: f64,
    h: f64,
    l: f64,
}
fn points(d: &Value) -> Vec<Pt> {
    let raw = d["points"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<Pt> = Vec::with_capacity(raw.len());
    for p in &raw {
        let Some(v) = p["value"].as_f64() else { continue };
        let prev = out.last().map(|x| x.v).unwrap_or(v);
        // Weekly bars may lack open/high/low: as the old chart, from the close before.
        let o = p["open"].as_f64().unwrap_or(prev);
        let spread = (v - o).abs() * 0.15;
        let h = p["high"].as_f64().unwrap_or(v.max(o) + spread).max(v.max(o));
        let l = p["low"].as_f64().unwrap_or(v.min(o) - spread).min(v.min(o));
        out.push(Pt { time: p["time"].as_str().unwrap_or("").to_string(), v, o, h, l });
    }
    out
}

/// "2026-11-25" → "25/11/2026"; intraday "2026-11-25 14:05" → "25/11 14:05".
fn date_long(t: &str) -> String {
    let (d, hm) = t.split_once(' ').unwrap_or((t, ""));
    let p: Vec<&str> = d.split('-').collect();
    if p.len() != 3 {
        return t.to_string();
    }
    if hm.is_empty() { format!("{}/{}/{}", p[2], p[1], p[0]) } else { format!("{}/{} {hm}", p[2], p[1]) }
}
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
/// An axis label for a range.
fn date_short(t: &str, range: &str) -> String {
    let (d, hm) = t.split_once(' ').unwrap_or((t, ""));
    let p: Vec<&str> = d.split('-').collect();
    if p.len() != 3 {
        return t.to_string();
    }
    let mon = p[1].parse::<usize>().ok().and_then(|m| MONTHS.get(m.wrapping_sub(1))).copied().unwrap_or("");
    match range {
        "1D" => hm.to_string(),
        "5D" => format!("{} {mon}", p[2].trim_start_matches('0')),
        "5Y" => format!("{mon} {}", p[0]),
        "MAX" => p[0].to_string(),
        _ => format!("{} {mon}", p[2].trim_start_matches('0')),
    }
}

/// Round steps for the price axis.
fn ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let raw = (hi - lo).max(1e-9) / n as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|k| k * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    let mut t = (lo / step).ceil() * step;
    let mut out = vec![];
    while t <= hi + 1e-9 && out.len() < 12 {
        out.push(t);
        t += step;
    }
    out
}

#[component]
pub fn ChartLayer() -> impl IntoView {
    let it = OPEN.with(|o| *o);
    move || it.get().map(|item| view! { <leptos::portal::Portal><Chart item=item.clone()/></leptos::portal::Portal> })
}

const W: f64 = 1000.0;
const H: f64 = 400.0;

#[component]
fn Chart(item: Item) -> impl IntoView {
    let range = RwSignal::new("1Y");
    let candle = RwSignal::new(false);
    let log = RwSignal::new(false);
    let hover = RwSignal::new(None::<usize>);
    let data = RwSignal::new(HashMap::<&'static str, Result<Value, String>>::new());
    let sym = item.symbol.clone();
    Effect::new(move |_| {
        let src = source(range.get());
        if data.with_untracked(|d| matches!(d.get(src), Some(Ok(_)))) {
            return;
        }
        data.update(|d| {
            d.remove(src);
        });
        let path = format!("/chart?symbol={}&range={src}", js_sys::encode_uri_component(&sym));
        spawn_local(async move {
            let r = api_get::<Value>(&path).await.map_err(|e| e.message);
            data.try_update(|d| {
                d.insert(src, r);
            });
        });
    });
    on_window(leptos::ev::keydown, move |e: web_sys::KeyboardEvent| {
        if e.key() == "Escape" {
            e.stop_propagation();
            close();
        }
    });
    let answer = Memo::new(move |_| data.with(|d| d.get(source(range.get())).cloned()));
    let full = Memo::new(move |_| answer.with(|a| a.as_ref().and_then(|a| a.as_ref().ok()).map(points).unwrap_or_default()));
    // The points the range shows.
    let pts = Memo::new(move |_| {
        let all = full.get();
        let Some(last) = all.last() else { return all };
        match from_day(range.get(), &last.time) {
            Some(from) => all.into_iter().filter(|p| p.time.as_str() >= from.as_str()).collect(),
            None => all,
        }
    });
    let info = Memo::new(move |_| answer.with(|a| a.as_ref().and_then(|a| a.as_ref().ok()).cloned().unwrap_or(Value::Null)));
    let up = item.change > 0.0;
    let down = item.change < 0.0;
    let main = if up { "var(--positive)" } else if down { "var(--negative)" } else { "var(--primary)" };
    // Price → y in the plot (0 at the top), on a linear or log scale.
    let scale = Memo::new(move |_| {
        let p = pts.get();
        let c = candle.get();
        let (mut lo, mut hi) = p.iter().fold((f64::MAX, f64::MIN), |(a, b), x| if c { (a.min(x.l), b.max(x.h)) } else { (a.min(x.v), b.max(x.v)) });
        // The close before (a day's bars start from it) is on the scale too.
        if let Some(pv) = info.with(|i| i["previousClose"].as_f64()).filter(|_| !c && !p.is_empty()) {
            (lo, hi) = (lo.min(pv), hi.max(pv));
        }
        if lo > hi {
            (lo, hi) = (0.0, 1.0);
        }
        let lg = log.get() && lo > 0.0;
        let f = move |x: f64| if lg { x.max(1e-9).ln() } else { x };
        let (a, b) = (f(lo), f(hi));
        let pad = (b - a).max(1e-9);
        (a - pad * 0.1, b + pad * 0.08, lg, lo, hi)
    });
    let y = move |price: f64| {
        let (a, b, lg, ..) = scale.get();
        let v = if lg { price.max(1e-9).ln() } else { price };
        H - (v - a) / (b - a) * H
    };
    // Candles keep half a bar clear at each end.
    let x = move |i: usize, n: usize| {
        let pad = if candle.get() { W / n.max(1) as f64 / 2.0 } else { 0.0 };
        if n > 1 { pad + i as f64 / (n - 1) as f64 * (W - 2.0 * pad) } else { W / 2.0 }
    };

    let plot = move || {
        let p = pts.get();
        let n = p.len();
        if n == 0 {
            return None;
        }
        let last = p[n - 1].v;
        let prev = info.with(|i| i["previousClose"].as_f64());
        let body = if candle.get() {
            let bw = (W / n as f64 * 0.65).max(0.6);
            p.iter().enumerate().map(|(i, q)| {
                let cx = x(i, n);
                let col = if q.v >= q.o { "var(--positive)" } else { "var(--negative)" };
                let (top, bot) = (y(q.v.max(q.o)), y(q.v.min(q.o)));
                view! {
                    <g fill=col stroke=col>
                        <line x1=f(cx) x2=f(cx) y1=f(y(q.h)) y2=f(y(q.l)) stroke-opacity="0.55" vector-effect="non-scaling-stroke"/>
                        <rect x=f(cx - bw / 2.0) y=f(top) width=f(bw) height=f((bot - top).max(0.8)) stroke="none"/>
                    </g>
                }
            }).collect_view().into_any()
        } else {
            let line: String = p.iter().enumerate().map(|(i, q)| format!("{:.2},{:.2}", x(i, n), y(q.v))).collect::<Vec<_>>().join(" ");
            let area = format!("M0,{H} L{} L{W},{H} Z", line.replace(' ', " L"));
            view! {
                <path d=area fill="url(#cc-fill)" stroke="none"/>
                <polyline points=line fill="none" stroke=main stroke-width="2" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>
                {prev.map(|pv| view! {
                    <line class="cc-prev" x1="0" x2=f(W) y1=f(y(pv)) y2=f(y(pv)) vector-effect="non-scaling-stroke"/>
                })}
            }.into_any()
        };
        Some(view! {
            {body}
            <line x1="0" x2=f(W) y1=f(y(last)) y2=f(y(last)) stroke=main stroke-opacity="0.45" stroke-dasharray="4 4" vector-effect="non-scaling-stroke"/>
            {move || hover.get().filter(|i| *i < n).map(|i| {
                let q = &p[i];
                view! {
                    <line class="cc-cross" x1=f(x(i, n)) x2=f(x(i, n)) y1="0" y2=f(H) vector-effect="non-scaling-stroke"/>
                    <line class="cc-cross" x1="0" x2=f(W) y1=f(y(q.v)) y2=f(y(q.v)) vector-effect="non-scaling-stroke"/>
                }
            })}
        })
    };
    // Price labels on the right, dates under the plot (HTML, so the text isn't stretched).
    let yaxis = move || {
        let (_, _, lg, lo, hi) = scale.get();
        let t = if lg { ticks(lo, hi, 4) } else { ticks(lo, hi, 5) };
        t.into_iter().map(|v| view! { <span style=format!("top:{:.2}%", y(v) / H * 100.0)>{price_text(v, None)}</span> }).collect_view()
    };
    let xaxis = move || {
        let p = pts.get();
        let n = p.len();
        let r = range.get();
        (n > 1).then(|| (0..6).map(|k| k * (n - 1) / 5).map(|i| view! { <span style=format!("left:{:.2}%", x(i, n) / W * 100.0)>{date_short(&p[i].time, r)}</span> }).collect_view())
    };
    let plot_ref = NodeRef::<leptos::html::Div>::new();
    let track = move |e: web_sys::PointerEvent| {
        let Some(el) = plot_ref.get_untracked() else { return };
        let r = el.get_bounding_client_rect();
        let n = pts.with_untracked(|p| p.len());
        if n == 0 || r.width() <= 0.0 {
            return;
        }
        let f = ((e.client_x() as f64 - r.left()) / r.width()).clamp(0.0, 1.0);
        hover.set(Some((f * (n - 1) as f64).round() as usize));
    };
    let sym2 = item.symbol.clone();
    let item_sym = move || sym2.clone();
    let tip = move || {
        let i = hover.get()?;
        let p = pts.get();
        let q = p.get(i)?.clone();
        let n = p.len();
        let left = x(i, n) / W * 100.0;
        let top = y(q.v) / H * 100.0;
        let side = if left > 70.0 { "right:calc(100% - ".to_string() + &format!("{left:.2}% + 14px)") } else { format!("left:calc({left:.2}% + 14px)") };
        Some(view! {
            <div class="cc-tooltip" style=format!("{side};top:calc({:.2}% + 14px)", top.min(75.0))>
                <div class="cc-tip-date">{date_long(&q.time)}</div>
                {if candle.get() {
                    view! {
                        <div class="cc-tip-row"><span class="cc-tip-label">"Open"</span><span class="cc-tip-value">{price_text(q.o, None)}</span></div>
                        <div class="cc-tip-row"><span class="cc-tip-label">"High"</span><span class="cc-tip-value">{price_text(q.h, None)}</span></div>
                        <div class="cc-tip-row"><span class="cc-tip-label">"Low"</span><span class="cc-tip-value">{price_text(q.l, None)}</span></div>
                        <div class="cc-tip-row"><span class="cc-tip-label">"Close"</span><span class="cc-tip-value">{price_text(q.v, None)}</span></div>
                    }.into_any()
                } else {
                    view! { <div class="cc-tip-row"><span class="cc-tip-label"><span class="cc-dot" style=format!("background:{main}")/>{item_sym()}</span><span class="cc-tip-value">{price_text(q.v, None)}</span></div> }.into_any()
                }}
            </div>
            <span class="cc-dot-at" style=format!("left:{left:.2}%;top:{top:.2}%;background:{main}")/>
        })
    };
    // The metrics bar: the price (or the hovered day's), the day's change, the last move, the range's.
    let latest = {
        let price = item.price.clone();
        move || match hover.get().and_then(|i| pts.with(|p| p.get(i).cloned())) {
            Some(q) => (date_long(&q.time), price_text(q.v, None)),
            None => ("Latest".to_string(), if price.is_empty() || price == "–" { pts.with(|p| p.last().map(|q| price_text(q.v, None)).unwrap_or_else(|| "—".into())) } else { price.clone() }),
        }
    };
    let latest2 = latest.clone();
    let change_abs = move || {
        let p = full.get();
        let last = p.last()?.v;
        let prev = info.with(|i| i["previousClose"].as_f64()).or_else(|| (p.len() > 1).then(|| p[p.len() - 2].v))?;
        Some(format!("{}{}", if last - prev >= 0.0 { "+" } else { "−" }, price_text((last - prev).abs(), Some(if last.abs() < 10.0 { 4 } else { 2 }))))
    };
    let in_range = move || {
        let p = pts.get();
        let (first, last) = (p.first()?.v, p.last()?.v);
        (first != 0.0).then(|| (last / first - 1.0) * 100.0)
    };
    let title = if item.name.is_empty() { item.symbol.clone() } else { item.name.clone() };
    let aria = format!("{title} chart");
    let named = !item.name.is_empty() && item.name != item.symbol;
    let sub_fallback = item.symbol.clone();
    let yahoo = format!("https://finance.yahoo.com/quote/{}", js_sys::encode_uri_component(&item.symbol));
    let day = item.change;
    view! {
        <div class="cerq-chart" role="dialog" aria-modal="true" aria-label=aria>
            <div class="cc-head">
                <div class="cc-title">
                    <h2>{move || info.with(|i| i["name"].as_str().filter(|n| !n.is_empty() && !named).map(str::to_string)).unwrap_or_else(|| title.clone())}</h2>
                    <span class="cc-sub">{move || {
                        let parts: Vec<String> = info.with(|i| ["symbol", "currency", "exchange"].iter().filter_map(|k| i[*k].as_str().map(str::to_string)).collect());
                        if parts.is_empty() { sub_fallback.clone() } else { parts.join(" · ") }
                    }}</span>
                </div>
                <button class="cc-close" type="button" aria-label="Close" title="Close (Esc)" on:click=move |_| close()><Icon name="x" size=16/></button>
            </div>
            <div class="cc-metrics">
                <div class="cc-metric"><span class="cc-metric-label">{move || latest().0}</span><span class="cc-metric-value" style=format!("color:{main}")>{move || latest2().1}</span></div>
                <div class="cc-metric"><span class="cc-metric-label">"Daily change"</span><span class={ if day > 0.0 { "cc-metric-value pos" } else if day < 0.0 { "cc-metric-value neg" } else { "cc-metric-value" } }>{format!("{}{:.2}%", if day >= 0.0 { "+" } else { "" }, day)}</span></div>
                <div class="cc-metric"><span class="cc-metric-label">"Change"</span><span class="cc-metric-value">{move || change_abs().unwrap_or_else(|| "—".into())}</span></div>
                <div class="cc-metric"><span class="cc-metric-label">"In range"</span>{move || match in_range() {
                    Some(p) => view! { <span class={ if p >= 0.0 { "cc-metric-value pos" } else { "cc-metric-value neg" } }>{format!("{} {:.2}%", if p >= 0.0 { "▲" } else { "▼" }, p.abs())}</span> }.into_any(),
                    None => view! { <span class="cc-metric-value">"—"</span> }.into_any(),
                }}</div>
            </div>
            <div class="cc-body">
                <div class="cc-controls">
                    <div class="cc-seg">
                        <button type="button" class:on=move || !candle.get() on:click=move |_| candle.set(false)>"AREA"</button>
                        <button type="button" class:on=move || candle.get() on:click=move |_| candle.set(true)>"CANDLE"</button>
                    </div>
                    <div class="cc-seg">
                        <button type="button" class:on=move || !log.get() on:click=move |_| log.set(false)>"LIN"</button>
                        <button type="button" class:on=move || log.get() on:click=move |_| log.set(true)>"LOG"</button>
                    </div>
                    <span class="cc-grow"/>
                    <div class="cc-seg cc-ranges" role="radiogroup" aria-label="Range">
                        {RANGES.into_iter().map(|r| view! {
                            <button type="button" role="radio" aria-checked=move || (range.get() == r).to_string() class:on=move || range.get() == r on:click=move |_| { hover.set(None); range.set(r) }>{r}</button>
                        }).collect_view()}
                    </div>
                </div>
                <div class="cc-chart">
                    {move || match answer.get() {
                        None => Some(view! { <div class="cc-status">"Loading…"</div> }.into_any()),
                        Some(Err(e)) => Some(view! { <div class="cc-status">{format!("Couldn’t load the chart: {e}")}</div> }.into_any()),
                        Some(Ok(_)) if pts.with(|p| p.is_empty()) => Some(view! { <div class="cc-status">"No data available"</div> }.into_any()),
                        Some(Ok(_)) => None,
                    }}
                    <div class="cc-plot" node_ref=plot_ref on:pointermove=track on:pointerdown=track on:pointerleave=move |_| hover.set(None)>
                        <svg viewBox=format!("0 0 {W} {H}") preserveAspectRatio="none" aria-hidden="true">
                            <defs>
                                <linearGradient id="cc-fill" x1="0" x2="0" y1="0" y2="1">
                                    <stop offset="0" stop-color=main stop-opacity="0.25"/>
                                    <stop offset="1" stop-color=main stop-opacity="0.02"/>
                                </linearGradient>
                            </defs>
                            {move || { let (_, _, lg, lo, hi) = scale.get(); (if lg { ticks(lo, hi, 4) } else { ticks(lo, hi, 5) }).into_iter().map(|v| view! { <line class="cc-grid" x1="0" x2=f(W) y1=f(y(v)) y2=f(y(v)) vector-effect="non-scaling-stroke"/> }).collect_view() }}
                            {plot}
                        </svg>
                        {tip}
                    </div>
                    <div class="cc-yaxis" aria-hidden="true">{yaxis}</div>
                    <div class="cc-xaxis" aria-hidden="true">{xaxis}</div>
                </div>
            </div>
            <div class="cc-foot">
                <a href=yahoo target="_blank" rel="noopener noreferrer">"Source: Yahoo Finance"</a>
                <span>{move || info.with(|i| i["updated"].as_f64()).map(|t| format!("Updated: {}", String::from(js_sys::Date::new(&(t * 1000.0).into()).to_locale_string("en-GB", &js_sys::Object::new())))).unwrap_or_default()}</span>
            </div>
        </div>
    }
}
