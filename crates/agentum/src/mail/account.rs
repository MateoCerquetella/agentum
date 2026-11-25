//! Mail accounts (the email_accounts table), which ones can be used, OAuth
//! access tokens (Google; Microsoft with its rotating refresh token) and
//! "your addresses".
use super::state;
use crate::{secrets, state::App, util::now_ts};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, sync::Mutex};

#[derive(Clone, Debug, Default)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_user: String,
    pub imap_starttls: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: String,
    pub smtp_user: String,
    pub from: String,
    pub display_name: String,
    /// Still encrypted ("enc:…"): decrypted only when connecting.
    pub imap_pw: String,
    pub smtp_pw: String,
    pub refresh: String,
    /// "google", "microsoft" or "".
    pub oauth: String,
    pub default: bool,
}

const MS_TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const MS_SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access";

fn env(k: &str) -> String { std::env::var(k).unwrap_or_default() }

impl Account {
    pub fn usable(&self) -> bool {
        if !(self.enabled && !self.imap_host.is_empty() && !self.imap_user.is_empty()) { return false; }
        match self.oauth.as_str() {
            "google" => !self.refresh.is_empty() && !env("GOOGLE_OAUTH_CLIENT_ID").is_empty() && !env("GOOGLE_OAUTH_CLIENT_SECRET").is_empty(),
            "microsoft" => !self.refresh.is_empty() && !env("MICROSOFT_OAUTH_CLIENT_ID").is_empty(),
            "" => !self.imap_pw.is_empty(),
            _ => false,
        }
    }
    pub fn is_gmail(&self) -> bool { self.imap_host.contains("gmail") || self.imap_host.contains("googlemail") }
    /// Gmail and Outlook keep a copy of what goes out through their SMTP;
    /// other servers (iCloud) need it appended to Sent.
    pub fn saves_sent(&self) -> bool { self.is_gmail() || self.imap_host.contains("office365") || self.imap_host.contains("outlook") }
    pub fn public(&self) -> Value {
        json!({"id": self.id, "name": self.name, "enabled": self.enabled, "imap_user": self.imap_user, "from": self.from, "default": self.default, "usable": self.usable()})
    }
}

pub async fn accounts(app: &App) -> anyhow::Result<Vec<Account>> {
    app.db.call(|c| {
        let mut st = c.prepare("SELECT id,name,enabled,imap_host,imap_port,imap_user,imap_starttls,smtp_host,smtp_port,smtp_security,smtp_user,
            from_address,display_name,imap_password,smtp_password,oauth_refresh_token,oauth_provider,is_default FROM email_accounts ORDER BY rowid")?;
        let rows = st.query_map([], |r| {
            let t = |i: usize| -> rusqlite::Result<String> { Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default()) };
            let n = |i: usize| -> rusqlite::Result<i64> { Ok(r.get::<_, Option<i64>>(i)?.unwrap_or(0)) };
            let imap_user = t(5)?;
            let from = t(11)?;
            Ok(Account {
                id: t(0)?, name: t(1)?, enabled: n(2)? != 0, imap_host: t(3)?,
                imap_port: match n(4)? { 0 => 993, p => p as u16 }, imap_starttls: n(6)? != 0,
                smtp_host: t(7)?, smtp_port: match n(8)? { 0 => 465, p => p as u16 },
                smtp_security: match t(9)? { s if s.is_empty() => "ssl".into(), s => s },
                smtp_user: t(10)?, from: if from.is_empty() { imap_user.clone() } else { from }, imap_user,
                display_name: t(12)?, imap_pw: t(13)?, smtp_pw: t(14)?, refresh: t(15)?, oauth: t(16)?, default: n(17)? != 0,
            })
        })?;
        rows.collect()
    }).await
}

pub async fn names(app: &App) -> HashMap<String, String> {
    accounts(app).await.unwrap_or_default().into_iter().map(|a| (a.id, a.name)).collect()
}

/// A usable account by id, for actions.
pub async fn account(app: &App, id: &str) -> anyhow::Result<Account> {
    accounts(app).await?.into_iter().find(|a| a.id == id && a.usable()).ok_or_else(|| anyhow::anyhow!("unknown or unusable account"))
}

/// Your addresses: every account's login and From, iCloud's sibling domains
/// (me.com / icloud.com / mac.com share one mailbox), and saved aliases.
pub fn my_addresses(accts: &[Account], aliases: &[String]) -> HashSet<String> {
    let mut out: HashSet<String> = aliases.iter().map(|x| x.to_lowercase()).collect();
    for a in accts {
        for x in [&a.imap_user, &a.from] {
            if !x.is_empty() { out.insert(x.to_lowercase()); }
        }
        if a.imap_host.contains("mail.me.com") {
            let local = a.imap_user.split('@').next().unwrap_or("").to_lowercase();
            for d in ["me.com", "icloud.com", "mac.com"] { out.insert(format!("{local}@{d}")); }
        }
    }
    out
}

pub async fn mine(app: &App) -> HashSet<String> {
    let st = state::get(app).await;
    my_addresses(&accounts(app).await.unwrap_or_default(), &st.aliases)
}

/// Access tokens, reused while more than two minutes remain.
static TOKENS: Mutex<Option<HashMap<String, (String, i64)>>> = Mutex::new(None);
static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// An access token for an OAuth account.
pub async fn token(app: &App, a: &Account) -> anyhow::Result<String> {
    let cached = || TOKENS.lock().unwrap().get_or_insert_with(HashMap::new).get(&a.id).filter(|(_, exp)| exp - 120 > now_ts()).map(|(t, _)| t.clone());
    if let Some(t) = cached() { return Ok(t); }
    // One refresh at a time, so a sync and an action never spend the same
    // Microsoft refresh token twice.
    let _one = REFRESH.lock().await;
    if let Some(t) = cached() { return Ok(t); }
    let d: Value = if a.oauth == "microsoft" { microsoft(app, a).await? } else {
        let form = [("client_id", env("GOOGLE_OAUTH_CLIENT_ID")), ("client_secret", env("GOOGLE_OAUTH_CLIENT_SECRET")),
            ("refresh_token", secrets::decrypt(&a.refresh)), ("grant_type", "refresh_token".into())];
        let r = app.http.post("https://oauth2.googleapis.com/token").form(&form).timeout(std::time::Duration::from_secs(20)).send().await?;
        token_answer(r).await?
    };
    let tok = d["access_token"].as_str().ok_or_else(|| anyhow::anyhow!("no access token in the answer"))?.to_string();
    let exp = now_ts() + d["expires_in"].as_i64().unwrap_or(3600);
    TOKENS.lock().unwrap().get_or_insert_with(HashMap::new).insert(a.id.clone(), (tok.clone(), exp));
    Ok(tok)
}

async fn token_answer(r: reqwest::Response) -> anyhow::Result<Value> {
    let st = r.status();
    let v: Value = r.json().await.unwrap_or(json!({}));
    if !st.is_success() {
        anyhow::bail!("token refresh: HTTP {} {}", st.as_u16(), v["error"].as_str().unwrap_or(""));
    }
    Ok(v)
}

/// Microsoft replaces the refresh token on every use, so the newest one is
/// kept (encrypted) in the state. The chain is tied to the token in the
/// accounts table by a hash: a fresh sign-in there restarts it.
async fn microsoft(app: &App, a: &Account) -> anyhow::Result<Value> {
    // Dev only: the live service owns the rotating chain; refreshing it from
    // a second machine would break it.
    if env("CERQ_SKIP_OUTLOOK") == "1" { anyhow::bail!("Microsoft accounts are skipped here (CERQ_SKIP_OUTLOOK)"); }
    let seed = hex::encode(Sha256::digest(a.refresh.as_bytes()))[..16].to_string();
    let st = state::get(app).await;
    let refresh = match st.oauth_chain.get(&a.id) {
        Some(c) if c.seed == seed && !c.token.is_empty() => secrets::decrypt(&c.token),
        _ => secrets::decrypt(&a.refresh),
    };
    let form = [("client_id", env("MICROSOFT_OAUTH_CLIENT_ID")), ("refresh_token", refresh), ("grant_type", "refresh_token".into()), ("scope", MS_SCOPE.into())];
    let r = app.http.post(MS_TOKEN_URL).form(&form).timeout(std::time::Duration::from_secs(20)).send().await?;
    let d = token_answer(r).await?;
    if let Some(new) = d["refresh_token"].as_str() {
        let chain = state::Chain { seed, token: secrets::encrypt(new) };
        state::update(app, |s| { s.oauth_chain.insert(a.id.clone(), chain); }).await?;
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icloud_siblings_and_aliases() {
        let a = Account { imap_host: "imap.mail.me.com".into(), imap_user: "Ana@icloud.com".into(), from: "ana@icloud.com".into(), ..Default::default() };
        let m = my_addresses(&[a], &["Old@Hotmail.com".into()]);
        for x in ["ana@icloud.com", "ana@me.com", "ana@mac.com", "old@hotmail.com"] { assert!(m.contains(x), "{x}"); }
    }
}
