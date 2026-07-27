use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use reqwest::{Client, Response, StatusCode, Url, redirect};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

use crate::config::{AtlassianEndpoints, ClientSecret};

const MAX_UPSTREAM_RESPONSE: usize = 512 * 1024;

#[derive(Clone)]
pub(crate) struct AtlassianClient {
    http: Client,
    client_id: Arc<str>,
    client_secret: Arc<ClientSecret>,
    callback_url: Url,
    endpoints: AtlassianEndpoints,
}

impl AtlassianClient {
    pub(crate) fn new(
        client_id: String,
        client_secret: ClientSecret,
        callback_url: Url,
        endpoints: AtlassianEndpoints,
    ) -> Result<Self, AtlassianError> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(redirect::Policy::none())
            .user_agent(concat!("agentum-jira-broker/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| AtlassianError::Unavailable)?;
        Ok(Self {
            http,
            client_id: Arc::from(client_id),
            client_secret: Arc::new(client_secret),
            callback_url,
            endpoints,
        })
    }

    pub(crate) fn authorization_url(&self, state: &str, scopes: &[&str]) -> Url {
        let mut url = self.endpoints.authorization.clone();
        url.query_pairs_mut()
            .append_pair("audience", "api.atlassian.com")
            .append_pair("client_id", &self.client_id)
            .append_pair("scope", &scopes.join(" "))
            .append_pair("redirect_uri", self.callback_url.as_str())
            .append_pair("state", state)
            .append_pair("response_type", "code")
            .append_pair("prompt", "consent");
        url
    }

    pub(crate) async fn exchange_code(&self, code: &str) -> Result<TokenGrant, AtlassianError> {
        let request = AuthorizationCodeRequest {
            grant_type: "authorization_code",
            client_id: &self.client_id,
            client_secret: self.client_secret.expose(),
            code,
            redirect_uri: self.callback_url.as_str(),
        };
        self.token_request(&request).await
    }

    pub(crate) async fn refresh(&self, token: &str) -> Result<TokenGrant, AtlassianError> {
        let request = RefreshTokenRequest {
            grant_type: "refresh_token",
            client_id: &self.client_id,
            client_secret: self.client_secret.expose(),
            refresh_token: token,
        };
        self.token_request(&request).await
    }

    async fn token_request<T: Serialize + ?Sized>(
        &self,
        request: &T,
    ) -> Result<TokenGrant, AtlassianError> {
        let response = self
            .http
            .post(self.endpoints.token.clone())
            .header("Accept", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|_| AtlassianError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(AtlassianError::Rejected);
        }
        let token: TokenResponse = limited_json(response).await?;
        if !token.token_type.eq_ignore_ascii_case("bearer")
            || token.access_token.trim().is_empty()
            || token.access_token.len() > 64 * 1024
            || token
                .refresh_token
                .as_ref()
                .is_none_or(|value| value.trim().is_empty() || value.len() > 64 * 1024)
            || token.expires_in <= 60
            || token.expires_in > 24 * 60 * 60
        {
            return Err(AtlassianError::Malformed);
        }
        Ok(TokenGrant {
            access_token: token.access_token,
            refresh_token: token.refresh_token.expect("validated refresh token"),
            expires_in: token.expires_in,
            scope: token.scope,
        })
    }

    pub(crate) async fn accessible_resources(
        &self,
        access_token: &str,
    ) -> Result<Vec<AtlassianResource>, AtlassianError> {
        let response = self
            .http
            .get(self.endpoints.accessible_resources.clone())
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| AtlassianError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(AtlassianError::Rejected);
        }
        limited_json(response).await
    }
}

#[derive(Serialize)]
struct AuthorizationCodeRequest<'a> {
    grant_type: &'static str,
    client_id: &'a str,
    client_secret: &'a str,
    code: &'a str,
    redirect_uri: &'a str,
}

#[derive(Serialize)]
struct RefreshTokenRequest<'a> {
    grant_type: &'static str,
    client_id: &'a str,
    client_secret: &'a str,
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    token_type: String,
    #[serde(default)]
    scope: Option<String>,
}

pub(crate) struct TokenGrant {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub scope: Option<String>,
}

impl Drop for TokenGrant {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AtlassianResource {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

async fn limited_json<T: for<'de> Deserialize<'de>>(
    response: Response,
) -> Result<T, AtlassianError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_UPSTREAM_RESPONSE as u64)
    {
        return Err(AtlassianError::Malformed);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AtlassianError::Unavailable)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_UPSTREAM_RESPONSE {
            bytes.zeroize();
            return Err(AtlassianError::Malformed);
        }
        bytes.extend_from_slice(&chunk);
    }
    let parsed = serde_json::from_slice(&bytes).map_err(|_| AtlassianError::Malformed);
    bytes.zeroize();
    parsed
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AtlassianError {
    #[error("Atlassian OAuth service is unavailable")]
    Unavailable,
    #[error("Atlassian rejected the OAuth request")]
    Rejected,
    #[error("Atlassian returned a malformed OAuth response")]
    Malformed,
}
