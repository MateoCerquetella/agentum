//! Dashboard widgets: the data behind each widget type (what Glance used to
//! fetch), plus the server-stored layout and widget settings that replace
//! cerq-api's state.json and the per-browser localStorage layout.
use crate::{state::App, util::*};
use axum::{extract::{Path, Query, State}, Json};
use futures::future::join_all;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (X11; Linux aarch64) cerq/1.0";

pub fn default_widgets() -> Value {
    json!([
      {"id":"calendar","type":"calendar","title":"Calendar","options":{}},
      {"id":"weather-home","type":"weather","title":"Weather","options":{"location":"London, United Kingdom","units":"metric","hour-format":"24h"}},
      {"id":"clock","type":"clock","title":"Clock","options":{"hour-format":"24h","timezones":[{"timezone":"Etc/UTC","label":"UTC"},{"timezone":"America/New_York","label":"New York"},{"timezone":"Europe/London","label":"London"},{"timezone":"Asia/Tokyo","label":"Tokyo"}]}},
      {"id":"brief","type":"brief","title":"Brief","options":{}},
      {"id":"services","type":"monitor","title":"Services","options":{"sites":[{"title":"GitHub","url":"https://github.com","icon":"si:github"}]}},
      {"id":"hacker-news","type":"hacker-news","title":"Hacker News","options":{"sort-by":"top","limit":20,"collapse-after":10}},
      {"id":"news","type":"rss","title":"News","options":{"limit":30,"collapse-after":13,"feeds":[{"url":"https://www.ft.com/technology?format=rss","title":"Financial Times"},{"url":"https://feeds.a.dj.com/rss/RSSMarketsMain.xml","title":"Wall Street Journal"}]}},
      {"id":"mail","type":"mail","title":"Mail","options":{}},
      {"id":"markets-index","type":"markets","title":"Index","options":{"markets":[{"symbol":"SPY","name":"S&P 500"},{"symbol":"DX-Y.NYB","name":"Dollar Index"}]}},
      {"id":"releases","type":"releases","title":"Software Releases","options":{"repositories":["portainer/portainer","louislam/uptime-kuma","vaultwarden/server","nextcloud/server","pi-hole/pi-hole","glanceapp/glance","NginxProxyManager/nginx-proxy-manager"],"collapse-after":5}},
      {"id":"markets-crypto","type":"markets","title":"Crypto","options":{"markets":[{"symbol":"BTC-USD","name":"Bitcoin"},{"symbol":"ETH-USD","name":"Ethereum"}]}},
      {"id":"markets-stocks","type":"markets","title":"Stocks","options":{"sort-by":"absolute-change","markets":[{"symbol":"NVDA","name":"NVIDIA"},{"symbol":"AAPL","name":"Apple"},{"symbol":"MSFT","name":"Microsoft"},{"symbol":"GOOGL","name":"Google"},{"symbol":"AMD","name":"AMD"},{"symbol":"AMZN","name":"Amazon"},{"symbol":"TSLA","name":"Tesla"},{"symbol":"META","name":"Meta"}]}},
      {"id":"reddit-selfhosted","type":"reddit","title":"r/selfhosted","options":{"subreddit":"selfhosted","collapse-after":8}},
      {"id":"reddit-homelab","type":"reddit","title":"r/homelab","options":{"subreddit":"homelab","collapse-after":8}},
      {"id":"homelab-news","type":"rss","title":"Homelab News","options":{"style":"horizontal-cards","limit":15,"collapse-after":3,"feeds":[{"url":"https://www.servethehome.com/feed/","title":"ServeTheHome"},{"url":"https://blog.linuxserver.io/feed","title":"LinuxServer.io"},{"url":"https://blog.cloudflare.com/rss/","title":"Cloudflare Blog"},{"url":"https://selfh.st/feed/","title":"selfh.st"}]}},
      {"id":"lobsters","type":"lobsters","title":"Lobsters","options":{"sort-by":"hot","limit":15,"collapse-after":6}},
      {"id":"todo","type":"todo","title":"To-do","options":{}},
      {"id":"search","type":"search","title":"Search","options":{"search-engine":"duckduckgo","new-tab":true}}
    ])
}

pub fn default_layout() -> Value {
    json!({"pages":[
      {"name":"Dashboard","columns":[
        {"size":"small","widgets":["calendar","weather-home","clock"]},
        {"size":"full","widgets":["brief","services","hacker-news","news"]},
        {"size":"small","widgets":["mail","markets-index","releases"]}]}
    ]})
}

/// The widget types the "+ Add widget" panel offers, with their option fields.
pub fn types() -> Value {
    json!({
      "markets": {"label":"Markets","fields":[{"key":"markets","kind":"items","label":"Symbols","fields":[{"key":"symbol","label":"Symbol","hint":"Yahoo Finance ticker, e.g. AAPL, BTC-USD, ^MERV, ARS=X"},{"key":"name","label":"Name"}]},{"key":"sort-by","kind":"choice","label":"Sort by","options":[["","As listed"],["change","Change %"],["absolute-change","Absolute change"]]}]},
      "watchlist": {"label":"Watchlist","fields":[{"key":"layout","kind":"choice","label":"Layout","options":[["list","One long list"],["sections","Sections"],["tabs","Tabs"]]},{"key":"order","kind":"choice","label":"Order","options":[["","As listed"],["az","A→Z"],["za","Z→A"]]},{"key":"categories","kind":"items","label":"Categories","fields":[{"key":"title","label":"Category"},{"key":"markets","kind":"items","label":"Symbols","fields":[{"key":"symbol","label":"Symbol"},{"key":"name","label":"Name"}]}]}]},
      "weather": {"label":"Weather","fields":[{"key":"location","label":"Location","hint":"City, Country"},{"key":"units","kind":"choice","label":"Units","options":[["metric","Metric (°C)"],["imperial","Imperial (°F)"]]},{"key":"hour-format","kind":"choice","label":"Hour format","options":[["24h","24h"],["12h","12h"]]}]},
      "clock": {"label":"World clock","fields":[{"key":"hour-format","kind":"choice","label":"Hour format","options":[["24h","24h"],["12h","12h"]]},{"key":"timezones","kind":"items","label":"Time zones","fields":[{"key":"timezone","label":"Time zone","hint":"IANA name, e.g. Europe/Madrid"},{"key":"label","label":"Label"}]}]},
      "rss": {"label":"RSS feeds","fields":[{"key":"style","kind":"choice","label":"Style","options":[["vertical-list","List"],["horizontal-cards","Cards"]]},{"key":"limit","kind":"number","label":"Items"},{"key":"collapse-after","kind":"number","label":"Show before 'more'"},{"key":"feeds","kind":"items","label":"Feeds","fields":[{"key":"url","label":"Feed URL"},{"key":"title","label":"Title"}]}]},
      "reddit": {"label":"Reddit","fields":[{"key":"subreddit","label":"Subreddit","hint":"Without r/"},{"key":"collapse-after","kind":"number","label":"Show before 'more'"}]},
      "hacker-news": {"label":"Hacker News","fields":[{"key":"sort-by","kind":"choice","label":"Sort by","options":[["top","Top"],["new","New"],["best","Best"]]},{"key":"limit","kind":"number","label":"Items"},{"key":"collapse-after","kind":"number","label":"Show before 'more'"}]},
      "lobsters": {"label":"Lobsters","fields":[{"key":"sort-by","kind":"choice","label":"Sort by","options":[["hot","Hot"],["new","New"]]},{"key":"limit","kind":"number","label":"Items"},{"key":"collapse-after","kind":"number","label":"Show before 'more'"}]},
      "releases": {"label":"Releases","fields":[{"key":"repositories","kind":"strings","label":"Repositories","hint":"owner/repo"},{"key":"collapse-after","kind":"number","label":"Show before 'more'"}]},
      "repository": {"label":"GitHub repo","fields":[{"key":"repository","label":"Repository","hint":"owner/repo"},{"key":"pull-requests-limit","kind":"number","label":"Pull requests"},{"key":"issues-limit","kind":"number","label":"Issues"},{"key":"commits-limit","kind":"number","label":"Commits"}]},
      "monitor": {"label":"Site monitor","fields":[{"key":"sites","kind":"items","label":"Sites","fields":[{"key":"title","label":"Name"},{"key":"url","label":"URL"},{"key":"check-url","label":"Check URL","hint":"Optional, when the address you open isn't the one to ping"},{"key":"icon","label":"Icon","hint":"si:<name> from simple-icons.org, e.g. si:docker"},{"key":"timeout","label":"Timeout","hint":"Default 3s"}]}]},
      "videos": {"label":"YouTube","fields":[{"key":"channels","kind":"strings","label":"Channel IDs","hint":"Starts with UC"},{"key":"limit","kind":"number","label":"Videos"}]},
      "bookmarks": {"label":"Bookmarks","fields":[{"key":"groups","kind":"items","label":"Groups","fields":[{"key":"title","label":"Group"},{"key":"links","kind":"items","label":"Links","fields":[{"key":"title","label":"Title"},{"key":"url","label":"URL"},{"key":"icon","label":"Icon","hint":"si:<name> from simple-icons.org"}]}]}]},
      "search": {"label":"Search","fields":[{"key":"search-engine","kind":"choice","label":"Engine","options":[["duckduckgo","DuckDuckGo"],["google","Google"],["bing","Bing"],["kagi","Kagi"],["startpage","Startpage"]]}]},
      "todo": {"label":"To-do","fields":[]},
      "calendar": {"label":"Calendar","fields":[]},
      "mail": {"label":"Mail numbers","fields":[]},
      "brief": {"label":"Morning brief","fields":[]},
      "usage": {"label":"Agent usage","fields":[]}
    })
}

async fn widgets_of(app: &App) -> anyhow::Result<Value> {
    Ok(app.db.kv_get("dash.widgets").await?.unwrap_or_else(default_widgets))
}

pub async fn get_dashboard(State(app): State<App>) -> R<Json<Value>> {
    let layout = app.db.kv_get("dash.layout").await?.unwrap_or_else(default_layout);
    Ok(Json(json!({"layout": layout, "widgets": widgets_of(&app).await?, "types": types()})))
}

pub async fn put_layout(State(app): State<App>, Json(v): Json<Value>) -> R<Json<Value>> {
    if !v.get("pages").map(|p| p.is_array()).unwrap_or(false) { return Err(bad("pages missing")); }
    app.db.kv_set("dash.layout", &v).await?;
    Ok(Json(json!({"ok": true})))
}
pub async fn reset_layout(State(app): State<App>) -> R<Json<Value>> {
    app.db.kv_set("dash.layout", &default_layout()).await?;
    Ok(Json(json!({"ok": true})))
}

/// Create or update one widget: {id?, type, title, options}.
pub async fn put_widget(State(app): State<App>, Json(w): Json<Value>) -> R<Json<Value>> {
    let ty = s(&w, "type");
    if types().get(&ty).is_none() { return Err(bad("unknown widget type")); }
    let mut all = widgets_of(&app).await?;
    let arr = all.as_array_mut().unwrap();
    let mut id = s(&w, "id");
    if id.is_empty() { id = format!("{}-{}", ty, &uuid::Uuid::new_v4().simple().to_string()[..6]); }
    let title: String = s(&w, "title").chars().take(60).collect();
    let item = json!({"id": id, "type": ty, "title": if title.is_empty() { types()[&ty]["label"].as_str().unwrap_or("").to_string() } else { title }, "options": w.get("options").cloned().unwrap_or(json!({}))});
    match arr.iter_mut().find(|x| s(x, "id") == id) { Some(x) => *x = item.clone(), None => arr.push(item.clone()) }
    app.db.kv_set("dash.widgets", &all).await?;
    app.cache.forget(&format!("w:{id}"));
    Ok(Json(item))
}
pub async fn delete_widget(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let mut all = widgets_of(&app).await?;
    all.as_array_mut().unwrap().retain(|x| s(x, "id") != id);
    app.db.kv_set("dash.widgets", &all).await?;
    Ok(Json(json!({"ok": true})))
}

/// GET /api/widgets/:id/data — the fetched content for one widget.
pub async fn widget_data(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let all = widgets_of(&app).await?;
    let w = all.as_array().unwrap().iter().find(|x| s(x, "id") == id).cloned().ok_or_else(not_found)?;
    let o = w["options"].clone();
    let ty = s(&w, "type");
    let key = format!("w:{id}:{}", o);
    let ttl = Duration::from_secs(match ty.as_str() { "markets" | "watchlist" => 300, "monitor" => 60, "weather" => 900, "releases" | "repository" => 3600, _ => 600 });
    let a = app.clone();
    let v = app.cache.get_or(&key, ttl, || async move {
        match ty.as_str() {
            "markets" => markets(&a, &o).await,
            "watchlist" => watchlist(&a, &o).await,
            "weather" => weather(&a, &o).await,
            "rss" => rss(&a, &o).await,
            "hacker-news" => hacker_news(&a, &o).await,
            "lobsters" => lobsters(&a, &o).await,
            "reddit" => reddit(&a, &o).await,
            "releases" => releases(&a, &o).await,
            "repository" => repository(&a, &o).await,
            "monitor" => monitor(&a, &o).await,
            "videos" => videos(&a, &o).await,
            _ => Ok(json!({})),
        }
    }).await?;
    Ok(Json(v))
}

async fn get_json(app: &App, url: &str) -> anyhow::Result<Value> {
    let mut r = app.http.get(url).header("User-Agent", UA).header("Accept", "application/json");
    if url.starts_with("https://api.github.com") {
        if let Ok(t) = std::env::var("GITHUB_TOKEN") { r = r.bearer_auth(t); }
    }
    let r = r.timeout(Duration::from_secs(12)).send().await?;
    if !r.status().is_success() { anyhow::bail!("{} returned {}", url, r.status()); }
    Ok(r.json().await?)
}

fn arr<'a>(v: &'a Value, k: &str) -> Vec<&'a Value> { v.get(k).and_then(|x| x.as_array()).map(|a| a.iter().collect()).unwrap_or_default() }
fn num(v: &Value, k: &str, d: i64) -> i64 { v.get(k).and_then(|x| x.as_i64()).unwrap_or(d) }

async fn quote(app: &App, symbol: &str) -> anyhow::Result<Value> {
    let url = format!("https://query1.finance.yahoo.com/v8/finance/chart/{}?range=1mo&interval=1d", urlencode(symbol));
    let d = get_json(app, &url).await?;
    let r = &d["chart"]["result"][0];
    let meta = &r["meta"];
    let closes: Vec<f64> = r["indicators"]["quote"][0]["close"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
    let price = meta["regularMarketPrice"].as_f64().or(closes.last().copied()).unwrap_or(0.0);
    let prev = meta["chartPreviousClose"].as_f64();
    let prev = if closes.len() > 1 { closes[closes.len() - 2] } else { prev.unwrap_or(price) };
    let change = if prev != 0.0 { (price - prev) / prev * 100.0 } else { 0.0 };
    Ok(json!({"symbol": symbol, "price": price, "currency": meta["currency"], "change": change, "abs_change": price - prev, "spark": closes}))
}

async fn markets(app: &App, o: &Value) -> anyhow::Result<Value> {
    let list = arr(o, "markets");
    let res = join_all(list.iter().map(|m| async move {
        let sym = s(m, "symbol");
        let mut q = quote(app, &sym).await.unwrap_or_else(|_| json!({"symbol": sym, "error": true}));
        q["name"] = json!(if s(m, "name").is_empty() { sym.clone() } else { s(m, "name") });
        q
    })).await;
    let mut res = res;
    match s(o, "sort-by").as_str() {
        "change" => res.sort_by(|a, b| b["change"].as_f64().unwrap_or(0.0).total_cmp(&a["change"].as_f64().unwrap_or(0.0))),
        "absolute-change" => res.sort_by(|a, b| b["change"].as_f64().unwrap_or(0.0).abs().total_cmp(&a["change"].as_f64().unwrap_or(0.0).abs())),
        _ => {}
    }
    Ok(json!({"markets": res}))
}

async fn watchlist(app: &App, o: &Value) -> anyhow::Result<Value> {
    let mut cats = vec![];
    for c in arr(o, "categories") {
        let mut m = markets(app, &json!({"markets": c["markets"], "sort-by": o["sort-by"]})).await?["markets"].as_array().cloned().unwrap_or_default();
        match s(o, "order").as_str() { "az" => m.sort_by_key(|x| s(x, "symbol")), "za" => { m.sort_by_key(|x| s(x, "symbol")); m.reverse() } _ => {} }
        cats.push(json!({"title": c["title"], "markets": m}));
    }
    Ok(json!({"categories": cats, "layout": s(o, "layout"), "labels": o["labels"].as_bool().unwrap_or(true)}))
}

/// GET /api/chart?symbol=&range=1d|5d|1y|5y|max — prices for the chart modal: 5-minute (1d) or
/// 15-minute (5d) bars, daily closes (1y, 5y) or weekly (max). Each point: `time` ("YYYY-MM-DD", or
/// "YYYY-MM-DD HH:MM" intraday, in the exchange's time), `t` (unix seconds), value/open/high/low/close.
#[derive(Deserialize)]
pub struct ChartQ { symbol: String, range: Option<String> }
pub async fn chart(State(app): State<App>, Query(q): Query<ChartQ>) -> R<Json<Value>> {
    let rng = q.range.unwrap_or_else(|| "1y".into());
    let (years, interval): (i64, &str) = match rng.as_str() { "1d" => (0, "5m"), "5d" => (0, "15m"), "1y" => (1, "1d"), "5y" => (5, "1d"), "max" => (60, "1wk"), _ => return Err(bad("bad range")) };
    let intraday = years == 0;
    let sym = q.symbol.clone();
    if sym.is_empty() || sym.len() > 20 { return Err(bad("bad symbol")); }
    let a = app.clone();
    let ttl = Duration::from_secs(if intraday { 120 } else { 600 });
    let v = app.cache.get_or(&format!("chart:{sym}:{rng}"), ttl, || async move {
        let now = now_ts();
        let url = if intraday {
            format!("https://query1.finance.yahoo.com/v8/finance/chart/{}?range={rng}&interval={interval}&includePrePost=false", urlencode(&sym))
        } else {
            format!("https://query1.finance.yahoo.com/v8/finance/chart/{}?period1={}&period2={}&interval={}&includePrePost=false",
                urlencode(&sym), (now - years * 366 * 86400).max(0), now, interval)
        };
        let d = get_json(&a, &url).await?;
        let r = &d["chart"]["result"][0];
        if r.is_null() { anyhow::bail!("no data for {sym}"); }
        let meta = &r["meta"];
        let off = meta["gmtoffset"].as_i64().unwrap_or(0);
        let ts = r["timestamp"].as_array().cloned().unwrap_or_default();
        let q0 = &r["indicators"]["quote"][0];
        let mut by_time = std::collections::BTreeMap::new();
        for (i, t) in ts.iter().enumerate() {
            let (Some(t), Some(c)) = (t.as_i64(), q0["close"][i].as_f64()) else { continue };
            let local = chrono::DateTime::from_timestamp(t + off, 0).unwrap();
            let key = local.format(if intraday { "%Y-%m-%d %H:%M" } else { "%Y-%m-%d" }).to_string();
            by_time.insert(key, json!({"time": null, "t": t, "value": c, "open": q0["open"][i], "high": q0["high"][i], "low": q0["low"][i], "close": c}));
        }
        let points: Vec<Value> = by_time.into_iter().map(|(d, mut v)| { v["time"] = json!(d); v }).collect();
        // The close before this one: yesterday's for a day's bars, else the second-to-last point.
        let prev = if intraday { meta.get("chartPreviousClose").or(meta.get("previousClose")).cloned().unwrap_or(Value::Null) }
            else if points.len() > 1 { points[points.len() - 2]["value"].clone() } else { Value::Null };
        Ok(json!({"symbol": meta["symbol"], "name": meta.get("longName").or(meta.get("shortName")).cloned().unwrap_or(json!(sym)),
            "currency": meta["currency"], "exchange": meta.get("fullExchangeName").cloned().unwrap_or(meta["exchangeName"].clone()),
            "price": meta["regularMarketPrice"], "previousClose": prev, "updated": meta["regularMarketTime"], "intraday": intraday, "points": points}))
    }).await?;
    Ok(Json(v))
}

pub async fn geocode(app: &App, location: &str) -> anyhow::Result<(f64, f64, String)> {
    let mut parts = location.splitn(2, ',').map(|x| x.trim());
    let city = parts.next().unwrap_or("");
    let country = parts.next().unwrap_or("").to_lowercase();
    let d = get_json(app, &format!("https://geocoding-api.open-meteo.com/v1/search?name={}&count=10&language=en&format=json", urlencode(city))).await?;
    let rs = arr(&d, "results");
    let hit = rs.iter().find(|r| country.is_empty() || s(r, "country").to_lowercase() == country || s(r, "country_code").to_lowercase() == country)
        .or(rs.first()).ok_or_else(|| anyhow::anyhow!("location not found: {location}"))?;
    Ok((hit["latitude"].as_f64().unwrap_or(0.0), hit["longitude"].as_f64().unwrap_or(0.0), format!("{}, {}", s(hit, "name"), s(hit, "country"))))
}

pub async fn forecast(app: &App, location: &str, imperial: bool) -> anyhow::Result<Value> {
    let (lat, lon, place) = geocode(app, location).await?;
    let unit = if imperial { "&temperature_unit=fahrenheit" } else { "" };
    let d = get_json(app, &format!("https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&timezone=auto&current=temperature_2m,apparent_temperature,weather_code,is_day,wind_speed_10m&hourly=temperature_2m,precipitation_probability,weather_code&daily=sunrise,sunset,temperature_2m_max,temperature_2m_min,weather_code,precipitation_probability_max&forecast_days=3{unit}")).await?;
    Ok(json!({"place": place, "current": d["current"], "hourly": d["hourly"], "daily": d["daily"], "timezone": d["timezone"]}))
}

async fn weather(app: &App, o: &Value) -> anyhow::Result<Value> {
    let loc = s(o, "location");
    forecast(app, if loc.is_empty() { "London, United Kingdom" } else { &loc }, s(o, "units") == "imperial").await
}

async fn fetch_feed(app: &App, url: &str, title: &str) -> anyhow::Result<Vec<Value>> {
    let body = app.http.get(url).header("User-Agent", UA).timeout(Duration::from_secs(12)).send().await?.bytes().await?;
    let f = feed_rs::parser::parse(&body[..])?;
    let src = if title.is_empty() { f.title.map(|t| t.content).unwrap_or_default() } else { title.to_string() };
    Ok(f.entries.into_iter().map(|e| {
        let link = e.links.first().map(|l| l.href.clone()).unwrap_or_default();
        let when = e.published.or(e.updated).map(|d| d.timestamp());
        let image = e.media.iter().flat_map(|m| m.thumbnails.iter().map(|t| t.image.uri.clone()).chain(m.content.iter().filter_map(|c| c.url.as_ref().map(|u| u.to_string())))).next();
        let desc = e.summary.map(|s| strip_tags(&s.content)).unwrap_or_default();
        json!({"title": e.title.map(|t| t.content).unwrap_or_default(), "url": link, "time": when, "source": src, "image": image, "description": desc.chars().take(240).collect::<String>()})
    }).collect())
}

async fn rss(app: &App, o: &Value) -> anyhow::Result<Value> {
    let feeds = arr(o, "feeds");
    let all = join_all(feeds.iter().map(|f| async move { fetch_feed(app, &s(f, "url"), &s(f, "title")).await.unwrap_or_default() })).await;
    let mut items: Vec<Value> = all.into_iter().flatten().collect();
    items.sort_by_key(|i| -i["time"].as_i64().unwrap_or(0));
    items.truncate(num(o, "limit", 25) as usize);
    Ok(json!({"items": items}))
}

async fn hacker_news(app: &App, o: &Value) -> anyhow::Result<Value> {
    let sort = match s(o, "sort-by").as_str() { "new" => "new", "best" => "best", _ => "top" };
    let ids = get_json(app, &format!("https://hacker-news.firebaseio.com/v0/{sort}stories.json")).await?;
    let ids: Vec<i64> = ids.as_array().map(|a| a.iter().filter_map(|x| x.as_i64()).take(num(o, "limit", 15) as usize).collect()).unwrap_or_default();
    let items = join_all(ids.iter().map(|id| async move { get_json(app, &format!("https://hacker-news.firebaseio.com/v0/item/{id}.json")).await.ok() })).await;
    Ok(json!({"items": items.into_iter().flatten().map(|i| json!({
        "title": i["title"], "url": i.get("url").cloned().unwrap_or(json!(format!("https://news.ycombinator.com/item?id={}", i["id"]))),
        "comments_url": format!("https://news.ycombinator.com/item?id={}", i["id"]), "score": i["score"], "comments": i["descendants"], "time": i["time"],
        "domain": url::Url::parse(i["url"].as_str().unwrap_or("")).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
    })).collect::<Vec<_>>()}))
}

async fn lobsters(app: &App, o: &Value) -> anyhow::Result<Value> {
    let path = if s(o, "sort-by") == "new" { "newest" } else { "hottest" };
    let d = get_json(app, &format!("https://lobste.rs/{path}.json")).await?;
    let items: Vec<Value> = d.as_array().cloned().unwrap_or_default().into_iter().take(num(o, "limit", 15) as usize).map(|i| json!({
        "title": i["title"], "url": if s(&i, "url").is_empty() { i["comments_url"].clone() } else { i["url"].clone() }, "comments_url": i["comments_url"],
        "score": i["score"], "comments": i["comment_count"], "time": chrono::DateTime::parse_from_rfc3339(&s(&i, "created_at")).ok().map(|d| d.timestamp()), "tags": i["tags"]
    })).collect();
    Ok(json!({"items": items}))
}

async fn reddit(app: &App, o: &Value) -> anyhow::Result<Value> {
    let sub = s(o, "subreddit");
    if sub.is_empty() || !sub.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { anyhow::bail!("bad subreddit"); }
    let d = get_json(app, &format!("https://www.reddit.com/r/{sub}/hot.json?limit=25&raw_json=1")).await?;
    let items: Vec<Value> = arr(&d["data"], "children").iter().map(|c| &c["data"]).filter(|p| !p["stickied"].as_bool().unwrap_or(false)).map(|p| json!({
        "title": p["title"], "url": if p["is_self"].as_bool().unwrap_or(false) { json!(format!("https://www.reddit.com{}", s(p, "permalink"))) } else { p["url"].clone() },
        "comments_url": format!("https://www.reddit.com{}", s(p, "permalink")), "score": p["score"], "comments": p["num_comments"], "time": p["created_utc"].as_f64().map(|x| x as i64), "flair": p["link_flair_text"]
    })).collect();
    Ok(json!({"items": items}))
}

async fn releases(app: &App, o: &Value) -> anyhow::Result<Value> {
    let repos: Vec<String> = arr(o, "repositories").iter().filter_map(|x| x.as_str().map(String::from)).collect();
    let mut items: Vec<Value> = join_all(repos.iter().map(|r| async move {
        let d = get_json(app, &format!("https://api.github.com/repos/{r}/releases/latest")).await.ok()?;
        Some(json!({"repo": r, "name": d["tag_name"], "url": d["html_url"], "time": chrono::DateTime::parse_from_rfc3339(&s(&d, "published_at")).ok().map(|d| d.timestamp())}))
    })).await.into_iter().flatten().collect();
    items.sort_by_key(|i| -i["time"].as_i64().unwrap_or(0));
    Ok(json!({"items": items}))
}

async fn repository(app: &App, o: &Value) -> anyhow::Result<Value> {
    let r = s(o, "repository");
    let base = format!("https://api.github.com/repos/{r}");
    let (u1, u2, u3) = (format!("{base}/pulls?state=open&per_page={}", num(o, "pull-requests-limit", 5).max(0)),
        format!("{base}/issues?state=open&per_page={}", num(o, "issues-limit", 3).max(0) * 2),
        format!("{base}/commits?per_page={}", num(o, "commits-limit", 3).max(0)));
    let (info, prs, issues, commits) = tokio::join!(get_json(app, &base), get_json(app, &u1), get_json(app, &u2), get_json(app, &u3));
    let info = info?;
    let pick = |v: anyhow::Result<Value>, n: i64, f: &dyn Fn(&Value) -> Value| -> Vec<Value> {
        v.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(f).take(n.max(0) as usize).collect()
    };
    let issues: Vec<Value> = issues.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().filter(|i| i.get("pull_request").is_none())
        .take(num(o, "issues-limit", 3).max(0) as usize).map(|i| json!({"title": i["title"], "url": i["html_url"]})).collect();
    Ok(json!({"name": info["full_name"], "url": info["html_url"], "stars": info["stargazers_count"], "forks": info["forks_count"], "open_issues": info["open_issues_count"],
        "pulls": pick(prs, num(o, "pull-requests-limit", 5), &|p| json!({"title": p["title"], "url": p["html_url"]})), "issues": issues,
        "commits": pick(commits, num(o, "commits-limit", 3), &|c| json!({"title": s(&c["commit"], "message").lines().next().unwrap_or(""), "url": c["html_url"], "author": c["commit"]["author"]["name"]}))}))
}

/// A duration as Glance writes it ("3s", "500ms", "1m"), or a number of seconds.
fn duration_of(v: &Value, default: Duration) -> Duration {
    if let Some(n) = v.as_f64() { return Duration::from_secs_f64(n.clamp(0.1, 60.0)); }
    let t = v.as_str().unwrap_or("").trim();
    let (num, unit) = t.split_at(t.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(t.len()));
    let Ok(n) = num.trim().parse::<f64>() else { return default };
    let secs = match unit { "ms" => n / 1000.0, "m" => n * 60.0, _ => n };
    Duration::from_secs_f64(secs.clamp(0.1, 60.0))
}

/// A status check asks "is it up?", not "is its certificate one the public roots know": a homelab's
/// own CA (https://*.lan) answers too. Signatures are still checked; the chain isn't.
#[derive(Debug)]
struct AnyCert(std::sync::Arc<rustls::crypto::CryptoProvider>);
impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(&self, _: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>], _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
fn monitor_http() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().expect("tls versions")
            .dangerous().with_custom_certificate_verifier(std::sync::Arc::new(AnyCert(provider))).with_no_client_auth();
        reqwest::Client::builder().tls_backend_preconfigured(tls).pool_max_idle_per_host(1).pool_idle_timeout(Duration::from_secs(4)).build().expect("monitor client")
    })
}

/// Each site's status, as Glance's monitor: the check URL (or the URL), up when it answers below 400
/// (or with one of `alt-status-codes`), within `timeout` (3 s by default).
async fn monitor(_app: &App, o: &Value) -> anyhow::Result<Value> {
    let sites = arr(o, "sites");
    let res = join_all(sites.iter().map(|site| async move {
        let check = if s(site, "check-url").is_empty() { s(site, "url") } else { s(site, "check-url") };
        let timeout = duration_of(&site["timeout"], Duration::from_secs(3));
        let t = std::time::Instant::now();
        let r = if check.starts_with("http") { Some(monitor_http().get(&check).header("User-Agent", UA).timeout(timeout).send().await) } else { None };
        let ms = t.elapsed().as_millis() as u64;
        let code = r.as_ref().and_then(|r| r.as_ref().ok()).map(|r| r.status().as_u16());
        let error = match &r {
            None => Some("bad url"),
            Some(Err(e)) if e.is_timeout() => Some("timeout"),
            Some(Err(_)) => Some("error"),
            Some(Ok(_)) => None,
        };
        let alt: Vec<u64> = arr(site, "alt-status-codes").iter().filter_map(|x| x.as_u64()).collect();
        let up = code.map(|c| c < 400 || alt.contains(&(c as u64))).unwrap_or(false);
        let ok = code.is_some_and(|c| (200..300).contains(&c) || alt.contains(&(c as u64)));
        json!({"title": s(site, "title"), "url": s(site, "url"), "icon": s(site, "icon"), "up": up, "ok": ok, "status": code, "error": error, "ms": ms})
    })).await;
    Ok(json!({"sites": res}))
}

/// Simple-icons logos that turned out not to exist, so a page doesn't ask the CDN on every load.
static ICON_MISSES: std::sync::Mutex<Option<std::collections::HashMap<String, std::time::Instant>>> = std::sync::Mutex::new(None);

/// GET /api/icons/si/{name}: a simple-icons logo (Glance's `icon: si:<name>`), fetched once from the
/// simple-icons CDN and kept in `<data>/icons/si/`. Black on transparent: the page tints it.
pub async fn simple_icon(State(app): State<App>, Path(name): Path<String>) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;
    let name = name.trim_end_matches(".svg").to_ascii_lowercase();
    if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let dir = std::path::Path::new(&app.cfg.data_dir).join("icons").join("si");
    let file = dir.join(format!("{name}.svg"));
    let svg = match tokio::fs::read(&file).await {
        Ok(b) => b,
        Err(_) => {
            let missed = ICON_MISSES.lock().unwrap().as_ref().and_then(|m| m.get(&name).copied()).is_some_and(|t| t.elapsed() < Duration::from_secs(3600));
            let got = if missed { None } else { fetch_icon(&app, &name).await };
            match got {
                Some(b) => {
                    let tmp = dir.join(format!(".{name}.tmp"));
                    if tokio::fs::create_dir_all(&dir).await.is_ok() && tokio::fs::write(&tmp, &b).await.is_ok() {
                        let _ = tokio::fs::rename(&tmp, &file).await;
                    }
                    b
                }
                None => {
                    ICON_MISSES.lock().unwrap().get_or_insert_with(Default::default).insert(name, std::time::Instant::now());
                    return (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "private, max-age=3600")]).into_response();
                }
            }
        }
    };
    ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "private, max-age=604800"),
      (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")], svg).into_response()
}
async fn fetch_icon(app: &App, name: &str) -> Option<Vec<u8>> {
    let url = format!("https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/{name}.svg");
    let r = app.http.get(&url).header("User-Agent", UA).timeout(Duration::from_secs(10)).send().await.ok()?;
    if !r.status().is_success() { return None; }
    let b = r.bytes().await.ok()?;
    let text = std::str::from_utf8(&b).ok()?.trim_start().to_ascii_lowercase();
    // A logo, nothing that runs.
    let handler = regex::Regex::new(r"\son[a-z]+\s*=").unwrap();
    let safe = b.len() < 64 * 1024 && text.starts_with("<svg") && !["<script", "javascript:", "<foreignobject", "<iframe"].iter().any(|x| text.contains(x)) && !handler.is_match(&text);
    safe.then(|| b.to_vec())
}

async fn videos(app: &App, o: &Value) -> anyhow::Result<Value> {
    let chans: Vec<String> = arr(o, "channels").iter().filter_map(|x| x.as_str().map(String::from)).collect();
    let all = join_all(chans.iter().map(|c| async move { fetch_feed(app, &format!("https://www.youtube.com/feeds/videos.xml?channel_id={c}"), "").await.unwrap_or_default() })).await;
    let mut items: Vec<Value> = all.into_iter().flatten().collect();
    items.sort_by_key(|i| -i["time"].as_i64().unwrap_or(0));
    items.truncate(num(o, "limit", 12) as usize);
    Ok(json!({"items": items}))
}

pub fn urlencode(x: &str) -> String { url::form_urlencoded::byte_serialize(x.as_bytes()).collect() }
pub fn strip_tags(h: &str) -> String {
    let re = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    html_escape::decode_html_entities(&re.replace_all(h, " ")).split_whitespace().collect::<Vec<_>>().join(" ")
}
