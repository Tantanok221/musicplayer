use super::Server;
use crate::{platform::NativeToken, smart_rules::SmartRules};
use reqwest::{Method, StatusCode, header::HeaderValue};
use serde_json::{Value, json};
use std::fmt;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeError {
    PasswordRequired,
    Message(&'static str),
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PasswordRequired => "Enter your Navidrome password to edit smart playlists",
            Self::Message(message) => message,
        })
    }
}

impl std::error::Error for NativeError {}

impl Server {
    pub async fn renew_native_session(&self, token: &mut NativeToken) -> Result<(), NativeError> {
        self.native_request(token, Method::GET, &["api", "keepalive", "session"], None)
            .await?;
        Ok(())
    }

    fn native_url(&self, segments: &[&str]) -> Result<Url, NativeError> {
        if !self.is_navidrome {
            return Err(NativeError::Message(
                "Smart playlist editing requires Navidrome",
            ));
        }
        let mut url = Url::parse(&self.account.url)
            .map_err(|_| NativeError::Message("Invalid Navidrome server address"))?;
        url.path_segments_mut()
            .map_err(|_| NativeError::Message("Invalid Navidrome server address"))?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }

    pub async fn native_login(&self, password: &str) -> Result<NativeToken, NativeError> {
        let response = self
            .native_http
            .post(self.native_url(&["auth", "login"])?)
            .json(&json!({"username":self.account.username,"password":password}))
            .send()
            .await
            .map_err(|_| NativeError::Message("Could not reach Navidrome to sign in"))?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(NativeError::Message(
                "Incorrect Navidrome username or password",
            ));
        }
        if !response.status().is_success() {
            return Err(NativeError::Message("Navidrome sign-in failed. Try again."));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| NativeError::Message("Navidrome returned an invalid login response"))?;
        let token = body["token"].as_str().ok_or(NativeError::Message(
            "Navidrome returned an invalid login response",
        ))?;
        NativeToken::new(token.into())
            .map_err(|_| NativeError::Message("Navidrome returned an invalid login response"))
    }

    async fn native_request(
        &self,
        token: &mut NativeToken,
        method: Method,
        path: &[&str],
        body: Option<Value>,
    ) -> Result<Value, NativeError> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.value()))
            .map_err(|_| NativeError::PasswordRequired)?;
        authorization.set_sensitive(true);
        let mut request = self
            .native_http
            .request(method, self.native_url(path)?)
            .header("x-nd-authorization", authorization);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await
            .map_err(|_| NativeError::Message("Could not reach Navidrome. The playlist may not have been saved; check before retrying."))?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(NativeError::PasswordRequired);
        }
        if let Some(header) = response.headers().get("x-nd-authorization") {
            let value = header
                .to_str()
                .map_err(|_| NativeError::Message("Navidrome returned an invalid login token"))?;
            *token = NativeToken::new(value.strip_prefix("Bearer ").unwrap_or(value).into())
                .map_err(|_| NativeError::Message("Navidrome returned an invalid login token"))?;
        }
        if !response.status().is_success() {
            return Err(NativeError::Message(match response.status() {
                StatusCode::FORBIDDEN => "Navidrome denied this playlist operation",
                StatusCode::NOT_FOUND => "This playlist no longer exists on Navidrome",
                _ => "Navidrome could not complete the playlist operation",
            }));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| NativeError::Message("Could not read the Navidrome response"))?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| NativeError::Message("Navidrome returned an invalid playlist response"))
    }

    pub async fn smart_rules(
        &self,
        token: &mut NativeToken,
        id: &str,
    ) -> Result<Option<SmartRules>, NativeError> {
        let body = self
            .native_request(token, Method::GET, &["api", "playlist", id], None)
            .await?;
        let rules = body.get("rules").ok_or(NativeError::Message(
            "Navidrome returned an invalid playlist response",
        ))?;
        serde_json::from_value(rules.clone())
            .map_err(|_| NativeError::Message("Navidrome returned invalid smart playlist rules"))
    }

    pub async fn create_smart_playlist(
        &self,
        token: &mut NativeToken,
        name: &str,
        rules: &SmartRules,
    ) -> Result<String, NativeError> {
        let body = self
            .native_request(
                token,
                Method::POST,
                &["api", "playlist"],
                Some(json!({"name":name,"comment":"","public":false,"rules":rules})),
            )
            .await?;
        body["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or(NativeError::Message(
                "Navidrome returned no playlist ID. Check your playlists before retrying.",
            ))
    }

    pub async fn update_smart_playlist(
        &self,
        token: &mut NativeToken,
        id: &str,
        name: &str,
        rules: &SmartRules,
    ) -> Result<(), NativeError> {
        self.native_request(
            token,
            Method::PUT,
            &["api", "playlist", id],
            Some(json!({"name":name,"rules":rules})),
        )
        .await?;
        Ok(())
    }
}
