use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use reqwest::{Client, Response, StatusCode, Url, redirect};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

#[cfg(test)]
use crate::config::AtlassianEndpoints;
use crate::config::ClientSecret;

const MAX_UPSTREAM_RESPONSE: usize = 512 * 1024;
const AUTHORIZATION_ENDPOINT: &str = "https://auth.atlassian.com/authorize";
#[cfg(not(test))]
const TOKEN_ENDPOINT: &str = "https://auth.atlassian.com/oauth/token";
#[cfg(not(test))]
const ACCESSIBLE_RESOURCES_ENDPOINT: &str =
    "https://api.atlassian.com/oauth/token/accessible-resources";

#[derive(Clone)]
pub(crate) struct AtlassianClient {
    http: Client,
    client_id: Arc<str>,
    client_secret: Arc<ClientSecret>,
    callback_url: Url,
    #[cfg(test)]
    endpoints: AtlassianEndpoints,
}

impl AtlassianClient {
    pub(crate) fn new(
        client_id: String,
        client_secret: ClientSecret,
        callback_url: Url,
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
            #[cfg(test)]
            endpoints: AtlassianEndpoints::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        client_id: String,
        client_secret: ClientSecret,
        callback_url: Url,
        endpoints: AtlassianEndpoints,
    ) -> Result<Self, AtlassianError> {
        validate_test_endpoints(&endpoints)?;
        let mut client = Self::new(client_id, client_secret, callback_url)?;
        client.endpoints = endpoints;
        Ok(client)
    }

    #[cfg(not(test))]
    fn authorization_endpoint(&self) -> Url {
        exact_atlassian_url(AUTHORIZATION_ENDPOINT)
    }

    #[cfg(test)]
    fn authorization_endpoint(&self) -> Url {
        self.endpoints.authorization.clone()
    }

    #[cfg(not(test))]
    fn token_endpoint(&self) -> Url {
        exact_atlassian_url(TOKEN_ENDPOINT)
    }

    #[cfg(test)]
    fn token_endpoint(&self) -> Url {
        self.endpoints.token.clone()
    }

    #[cfg(not(test))]
    fn accessible_resources_endpoint(&self) -> Url {
        exact_atlassian_url(ACCESSIBLE_RESOURCES_ENDPOINT)
    }

    #[cfg(test)]
    fn accessible_resources_endpoint(&self) -> Url {
        self.endpoints.accessible_resources.clone()
    }

    pub(crate) fn authorization_url(&self, state: &str, scopes: &[&str]) -> Url {
        let mut url = self.authorization_endpoint();
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
            .post(self.token_endpoint())
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
            .get(self.accessible_resources_endpoint())
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

#[cfg(not(test))]
fn exact_atlassian_url(raw: &'static str) -> Url {
    let url = Url::parse(raw).expect("compiled Atlassian endpoint is valid");
    debug_assert!(
        (url.scheme() == "https")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    );
    url
}

#[cfg(test)]
fn validate_test_endpoints(endpoints: &AtlassianEndpoints) -> Result<(), AtlassianError> {
    if endpoints.authorization.as_str() != AUTHORIZATION_ENDPOINT
        || !is_loopback_test_endpoint(&endpoints.token, "/oauth/token")
        || !is_loopback_test_endpoint(&endpoints.accessible_resources, "/resources")
        || endpoints.token.origin() != endpoints.accessible_resources.origin()
    {
        return Err(AtlassianError::Malformed);
    }
    Ok(())
}

#[cfg(test)]
fn is_loopback_test_endpoint(url: &Url, expected_path: &str) -> bool {
    url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|address| address.is_loopback())
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == expected_path
        && url.query().is_none()
        && url.fragment().is_none()
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    fn endpoints() -> AtlassianEndpoints {
        AtlassianEndpoints {
            authorization: Url::parse(AUTHORIZATION_ENDPOINT).unwrap(),
            token: Url::parse("http://127.0.0.1:40123/oauth/token").unwrap(),
            accessible_resources: Url::parse("http://127.0.0.1:40123/resources").unwrap(),
        }
    }

    #[test]
    fn accepts_only_same_origin_loopback_test_upstreams() {
        assert!(validate_test_endpoints(&endpoints()).is_ok());

        let mut value = endpoints();
        value.token = Url::parse("http://[::1]:40123/oauth/token").unwrap();
        value.accessible_resources = Url::parse("http://[::1]:40123/resources").unwrap();
        assert!(validate_test_endpoints(&value).is_ok());
    }

    #[test]
    fn rejects_non_loopback_or_malformed_test_upstreams() {
        let hostile_values = [
            "https://auth.atlassian.com.evil.example/oauth/token",
            "http://example.com:40123/oauth/token",
            "http://user@127.0.0.1:40123/oauth/token",
            "http://127.0.0.1/oauth/token",
            "http://127.0.0.1:40123/wrong",
            "http://127.0.0.1:40123/oauth/token?redirect=evil",
        ];
        for hostile in hostile_values {
            let mut value = endpoints();
            value.token = Url::parse(hostile).unwrap();
            assert!(
                validate_test_endpoints(&value).is_err(),
                "unexpectedly accepted {hostile}"
            );
        }

        let mut split_origin = endpoints();
        split_origin.accessible_resources = Url::parse("http://127.0.0.1:40124/resources").unwrap();
        assert!(validate_test_endpoints(&split_origin).is_err());

        let mut authorization = endpoints();
        authorization.authorization =
            Url::parse("https://auth.atlassian.com.evil.example/authorize").unwrap();
        assert!(validate_test_endpoints(&authorization).is_err());
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
