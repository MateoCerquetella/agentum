//! Rule-based categories (cerq-mail's categorize): from the headers only.
use super::cache::Msg;
use regex::Regex;
use std::{collections::HashSet, sync::LazyLock};

pub const CATEGORIES: [&str; 9] = ["personal", "to_reply", "newsletter", "marketing", "receipt", "calendar", "notification", "otp", "cold_email"];

fn rx(s: &str) -> Regex { Regex::new(s).unwrap() }

static OTP: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)\b(verification code|verify your|one[- ]time|otp|2fa|two[- ]factor|security code|login code|sign[- ]in code|c[oó]digo de (verificaci[oó]n|seguridad|acceso)|tu c[oó]digo|your code|confirm your (email|account))\b"));
static RECEIPT: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)\b(receipt|recibo|factura|invoice|order (confirmation|#|number)|your order|pedido|compra|payment|pago (recibido|aprobado|realizado)|purchase|transaction|transferencia|comprobante|subscription renewed)\b"));
static CALENDAR: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)\b(invitation|invitaci[oó]n|accepted:|declined:|tentative:|updated invitation|event (reminder|update)|meeting (request|reminder)|calendar)\b"));
static MARKETING: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)(\d+\s?% (off|de descuento)|\bsale\b|\bdescuento|\boferta|\bdeal(s)?\b|\bpromo|black friday|cyber monday|free shipping|env[ií]o gratis|limited time|last chance|[uú]ltim[ao]s? (horas|d[ií]as)|exclusive offer)"));
static COLD: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)\b(quick question|partnership|collaborat|book a (call|demo|meeting)|15[- ]min|schedule a (call|meeting)|grow your|generate (more )?leads|lead generation|seo (services|agency)|outsourc|our agency|i came across|i noticed (your|that)|would you be (open|interested)|are you the right person|introduc(e|ing) (myself|our))\b"));
pub static NOREPLY: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)(^|[._+-])(no-?reply|do-?not-?reply|no-?responder|notifications?|notificaciones?|alerts?|alertas?|avisos?|mailer-daemon|bounces?|news|newsletter|novedades|info|updates?|marketing|ventas|hello|team|support|soporte|billing|receipts?|orders?|accounts?|security|events?|eventos?|service|servicio|clientes|budgets?|digest)([._+-]|@)"));
static BULK_SUBDOMAIN: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)@(info|infoemails?|news|newsletter|mail|email|emails|em|e|mkt|marketing|promo|offers|updates|notify|notifications|notificaciones|avisos|alertas|alerts|costalerts|ventas|comms|hello|go|send|mailer|bounce|reply)\."));
static ALERTS: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)\b(security|alert|sign[- ]in|signed in|password|account|privacy|terms|policy|verify|access|login|seguridad|alerta|contrase[nñ]a|cuenta|inicio de sesi[oó]n|privacidad|t[eé]rminos)\b"));
pub static FREEMAIL: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)@(gmail|googlemail|hotmail|outlook|live|msn|icloud|me|mac|yahoo|proton|protonmail|pm|aol|gmx|zoho|fastmail|tutanota|tuta|hey)\."));
pub static BULK_SYSTEMS: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)(hubspotemail|list-manage|mailchimp|sendgrid|mcsv|sparkpost|amazonses|bounce|unsubscribe|mailgun|rsgsv|exacttarget|salesforce|mktomail|klaviyo)"));
static NEWS_DOMAINS: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)(substack\.com|beehiiv\.com|mailchimp|convertkit|sendgrid|mailerlite|medium\.com|ghost\.io|buttondown|revue)"));

/// `known`: everyone you've written to; `mine`: your own addresses.
pub fn categorize(m: &Msg, known: &HashSet<String>, mine: &HashSet<String>) -> &'static str {
    let (subj, frm) = (m.subject.as_str(), m.from_addr.as_str());
    let bulk = !m.list_unsub.is_empty() || !m.list_id.is_empty() || matches!(m.precedence.as_str(), "bulk" | "list" | "junk") || BULK_SUBDOMAIN.is_match(frm);
    let automated = NOREPLY.is_match(frm) || (!m.auto_submitted.is_empty() && m.auto_submitted != "no");
    if m.ctype == "text/calendar" || (CALENDAR.is_match(subj) && (automated || frm.contains("calendar") || frm.contains("google.com"))) {
        return "calendar";
    }
    if OTP.is_match(subj) { return "otp"; }
    let domain = frm.rsplit('@').next().unwrap_or("");
    if RECEIPT.is_match(subj) && (automated || bulk || !matches!(domain, "gmail.com" | "hotmail.com" | "outlook.com" | "icloud.com")) {
        return "receipt";
    }
    if bulk && MARKETING.is_match(subj) { return "marketing"; }
    // Account and security mail, even when it has a List-Unsubscribe.
    if automated && ALERTS.is_match(subj) { return "notification"; }
    if bulk || NEWS_DOMAINS.is_match(frm) { return "newsletter"; }
    if automated { return "notification"; }
    if mine.contains(frm) { return "personal"; }
    if !known.contains(frm) && COLD.is_match(subj) { return "cold_email"; }
    "personal"
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(from: &str, subject: &str) -> Msg { Msg { from_addr: from.into(), subject: subject.into(), ..Default::default() } }
    fn cat(x: &Msg) -> &'static str { categorize(x, &HashSet::new(), &HashSet::new()) }

    #[test]
    fn categories() {
        assert_eq!(cat(&m("calendar-notification@google.com", "Invitation: Lunch @ Fri")), "calendar");
        assert_eq!(cat(&Msg { ctype: "text/calendar".into(), ..m("ana@gmail.com", "hola") }), "calendar");
        assert_eq!(cat(&m("ana@gmail.com", "Tu código de verificación")), "otp");
        assert_eq!(cat(&m("no-reply@shop.com", "Your order #1234")), "receipt");
        // A friend on Gmail writing about a "factura" is a person, not a receipt.
        assert_eq!(cat(&m("ana@gmail.com", "la factura")), "personal");
        assert_eq!(cat(&m("ana@empresa.com", "la factura")), "receipt");
        assert_eq!(cat(&Msg { list_unsub: "<https://x>".into(), ..m("deals@shop.com", "50% off everything") }), "marketing");
        assert_eq!(cat(&Msg { list_unsub: "<https://x>".into(), ..m("security@bank.com", "New sign-in to your account") }), "notification");
        assert_eq!(cat(&m("ana@acme.substack.com", "Weekly")), "newsletter");
        assert_eq!(cat(&m("ana@news.acme.com", "Weekly")), "newsletter");
        assert_eq!(cat(&m("alerts@bank.com", "Statement")), "notification");
        assert_eq!(cat(&m("bob@agency.io", "Quick question about your site")), "cold_email");
        let known: HashSet<String> = ["bob@agency.io".to_string()].into();
        assert_eq!(categorize(&m("bob@agency.io", "Quick question about your site"), &known, &HashSet::new()), "personal");
        assert_eq!(cat(&Msg { auto_submitted: "auto-replied".into(), ..m("bob@x.com", "Re: hi") }), "notification");
        assert_eq!(cat(&Msg { auto_submitted: "no".into(), ..m("bob@x.com", "Re: hi") }), "personal");
        // banks, clouds and shops that write like people (seen in the Outlook inbox)
        for from in ["alertas@infomistarjetas.com", "bancon@avisos.bancor.com.ar", "budgets@costalerts.amazonaws.com", "azure@infoemails.microsoft.com",
                     "events@ontidwit.com", "service@meliopayments.com", "smiles@ventas.smiles.com.ar"] {
            assert_ne!(cat(&m(from, "Novedades de tu cuenta")), "personal", "{from}");
        }
        assert_eq!(cat(&m("nathaly.delcarpio@insightglobal.com", "RE: Resume Update")), "personal");
    }
}
