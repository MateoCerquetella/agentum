//! The fixed words of Today, the wrap-up, the week ahead and the calendar
//! summaries in the language chosen in Settings (CERQ_BRIEF_LANG), as
//! i18n.py. The model writes its own sentences in that language.
use chrono::{Datelike, NaiveDate};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang { En, Es }
impl Lang {
    pub fn of(app: &crate::state::App) -> Lang { if app.lang_es() { Lang::Es } else { Lang::En } }
    pub fn code(self) -> &'static str { match self { Lang::En => "en", Lang::Es => "es" } }
    /// For prompts: "Write in English".
    pub fn name(self) -> &'static str { match self { Lang::En => "English", Lang::Es => "Spanish" } }
}

const DOW_EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DOW_ES: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
const DOWL_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const DOWL_ES: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
pub const DOW_MIN_EN: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
pub const DOW_MIN_ES: [&str; 7] = ["Lu", "Ma", "Mi", "Ju", "Vi", "Sá", "Do"];
const MONTH_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MONTH_ES: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
const MON_ES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];

fn table(l: Lang, key: &str) -> Option<&'static str> {
    let en = match key {
        "morning" => "Good morning", "afternoon" => "Good afternoon", "evening" => "Good evening",
        "today" => "today", "Today" => "Today", "Tomorrow" => "Tomorrow", "tomorrow" => "tomorrow", "in_days" => "in {n} days",
        "now_until" => "now · until {t}", "Now_until" => "Now · until {t}", "all_day" => "all day",
        "free_until" => "free until then ({d})", "busy" => "Busy {a}–{b}", "back_to_back" => "{n} things back to back",
        "nothing_else" => "Nothing else today", "free_day" => "Free day", "tomorrow_first" => "tomorrow {t} · {title} · {n} things",
        "tomorrow_free" => "tomorrow is free too", "rain" => "Rain {a}–{b}", "umbrella" => "umbrella", "during" => " · during {x}",
        "hot" => "Hot: {t}°", "hot_tip" => "water, light clothes", "cold" => "Cold: down to {t}°", "cold_tip" => "take a jacket",
        "waiting_1" => "{n} person is waiting for a reply", "waiting_n" => "{n} people are waiting for a reply",
        "oldest" => "oldest {n} days", "long_weekend" => "Long weekend",
        "wrap_umbrella" => "Leave an umbrella by the door: {p}% rain tomorrow.",
        "wrap_first" => "Tomorrow starts at {t} with {title}: get what you need ready tonight.",
        "wrap_easy" => "Nothing early tomorrow: an easy night.",
        "push_plan" => "Plan for today", "push_plan_body" => "Open Today to put them in your calendar.",
        "push_tonight" => "Tonight", "push_done" => "{n} done", "push_slipped" => "{n} slipped", "push_tomorrow" => "tomorrow {t} {title}",
        "push_week" => "Your week ahead", "push_leave" => "Leave by {t} for {title}", "push_leave_body" => "{t} at {place} · ~{m} min",
        "push_rain" => " · rain: take an umbrella", "own_birthday" => "Your own birthday",
        "w_today" => "today", "w_left" => "{n} left", "w_none" => "Nothing coming up.",
        "range_week" => "{a} – {b}",
        _ => return None,
    };
    if l == Lang::En { return Some(en); }
    Some(match key {
        "morning" => "Buen día", "afternoon" => "Buenas tardes", "evening" => "Buenas noches",
        "today" => "hoy", "Today" => "Hoy", "Tomorrow" => "Mañana", "tomorrow" => "mañana", "in_days" => "en {n} días",
        "now_until" => "ahora · hasta las {t}", "Now_until" => "Ahora · hasta las {t}", "all_day" => "todo el día",
        "free_until" => "libre hasta entonces ({d})", "busy" => "Ocupado {a}–{b}", "back_to_back" => "{n} cosas seguidas",
        "nothing_else" => "Nada más por hoy", "free_day" => "Día libre", "tomorrow_first" => "mañana {t} · {title} · {n} cosas",
        "tomorrow_free" => "mañana también está libre", "rain" => "Lluvia {a}–{b}", "umbrella" => "paraguas", "during" => " · durante {x}",
        "hot" => "Calor: {t}°", "hot_tip" => "agua y ropa liviana", "cold" => "Frío: hasta {t}°", "cold_tip" => "llevá abrigo",
        "waiting_1" => "{n} persona espera tu respuesta", "waiting_n" => "{n} personas esperan tu respuesta",
        "oldest" => "la más vieja, de hace {n} días", "long_weekend" => "Fin de semana largo",
        "wrap_umbrella" => "Dejá un paraguas en la puerta: {p}% de lluvia mañana.",
        "wrap_first" => "Mañana arrancás a las {t} con {title}: dejá listo lo que necesites.",
        "wrap_easy" => "Nada temprano mañana: una noche tranquila.",
        "push_plan" => "Plan para hoy", "push_plan_body" => "Abrí Hoy para pasarlos al calendario.",
        "push_tonight" => "Esta noche", "push_done" => "{n} hechas", "push_slipped" => "{n} pendientes", "push_tomorrow" => "mañana {t} {title}",
        "push_week" => "Tu semana", "push_leave" => "Salí a las {t} para {title}", "push_leave_body" => "{t} en {place} · ~{m} min",
        "push_rain" => " · llueve: llevá paraguas", "own_birthday" => "Tu propio cumpleaños",
        "w_today" => "hoy", "w_left" => "quedan {n}", "w_none" => "Nada por delante.",
        _ => en,
    })
}

/// The text for `key` with {name} placeholders filled.
pub fn t(l: Lang, key: &str, vals: &[(&str, &str)]) -> String {
    let mut s = table(l, key).unwrap_or(key).to_string();
    for (k, v) in vals { s = s.replace(&format!("{{{k}}}"), v); }
    s
}
pub fn t0(l: Lang, key: &str) -> String { t(l, key, &[]) }

pub fn dow(l: Lang, d: NaiveDate) -> &'static str { let i = d.weekday().num_days_from_monday() as usize; if l == Lang::Es { DOW_ES[i] } else { DOW_EN[i] } }
pub fn dow_long(l: Lang, d: NaiveDate) -> &'static str { let i = d.weekday().num_days_from_monday() as usize; if l == Lang::Es { DOWL_ES[i] } else { DOWL_EN[i] } }
pub fn month(l: Lang, d: NaiveDate) -> &'static str { let i = d.month0() as usize; if l == Lang::Es { MONTH_ES[i] } else { MONTH_EN[i] } }
pub fn mon(l: Lang, d: NaiveDate) -> &'static str { let i = d.month0() as usize; if l == Lang::Es { MON_ES[i] } else { &MONTH_EN[i][..3] } }
/// Monday 16 November / lunes 16 de noviembre
pub fn long_date(l: Lang, d: NaiveDate) -> String {
    if l == Lang::Es { format!("{} {} de {}", dow_long(l, d), d.day(), month(l, d)) } else { format!("{} {} {}", dow_long(l, d), d.day(), month(l, d)) }
}
/// Mon 16 Nov / lun 16 nov
pub fn short_date(l: Lang, d: NaiveDate) -> String { format!("{} {} {}", dow(l, d), d.day(), mon(l, d)) }
pub fn month_year(l: Lang, d: NaiveDate) -> String { format!("{} {}", month(l, d), d.year()) }
pub fn capitalize(s: &str) -> String { let mut c = s.chars(); match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() } }

/// Open-Meteo's WMO weather codes in words.
pub fn wmo(code: i64) -> &'static str {
    match code {
        0 => "clear", 1 => "mostly clear", 2 => "partly cloudy", 3 => "overcast", 45 | 48 => "fog",
        51 => "light drizzle", 53 => "drizzle", 55 => "heavy drizzle", 56 | 57 => "freezing drizzle",
        61 => "light rain", 63 => "rain", 65 => "heavy rain", 66 | 67 => "freezing rain",
        71 => "light snow", 73 => "snow", 75 => "heavy snow", 77 => "snow grains", 80 | 81 => "showers",
        82 => "violent showers", 85 | 86 => "snow showers", 95 => "thunderstorm", 96 | 99 => "thunderstorm with hail", _ => "",
    }
}
pub fn sky(l: Lang, words: &str) -> String {
    if l == Lang::En { return words.to_string(); }
    match words {
        "clear" => "despejado", "mostly clear" => "mayormente despejado", "partly cloudy" => "parcialmente nublado", "overcast" => "nublado",
        "fog" => "niebla", "light drizzle" => "llovizna leve", "drizzle" => "llovizna", "heavy drizzle" => "llovizna intensa",
        "freezing drizzle" => "llovizna helada", "light rain" => "lluvia leve", "rain" => "lluvia", "heavy rain" => "lluvia intensa",
        "freezing rain" => "lluvia helada", "light snow" => "nevada leve", "snow" => "nieve", "heavy snow" => "nevada intensa",
        "snow grains" => "granizo fino", "showers" => "chaparrones", "violent showers" => "chaparrones fuertes",
        "snow showers" => "chaparrones de nieve", "thunderstorm" => "tormenta", "thunderstorm with hail" => "tormenta con granizo",
        w => w,
    }.to_string()
}
