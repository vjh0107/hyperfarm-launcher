use std::time::Duration;

use reqwest::{Client, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::LauncherError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("{url} answered {status}")]
    Status {
        url: String,
        status: u16,
        body: String,
    },
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    #[error("{url} sent unreadable JSON: {source}")]
    Decode {
        url: String,
        source: serde_json::Error,
    },
}

impl HttpError {
    pub fn status(&self) -> Option<u16> {
        match self {
            HttpError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn body(&self) -> &str {
        match self {
            HttpError::Status { body, .. } => body,
            _ => "",
        }
    }

    pub fn without_url(self) -> Self {
        const HIDDEN: &str = "the request";
        match self {
            HttpError::Status { status, body, .. } => HttpError::Status {
                url: HIDDEN.into(),
                status,
                body,
            },
            HttpError::Transport(error) => HttpError::Transport(error.without_url()),
            HttpError::Decode { source, .. } => HttpError::Decode {
                url: HIDDEN.into(),
                source,
            },
        }
    }
}

impl From<HttpError> for LauncherError {
    fn from(error: HttpError) -> Self {
        log::warn!("{error}");
        LauncherError::network()
    }
}

#[derive(Clone)]
pub struct Http {
    client: Client,
}

impl Http {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(concat!("hyperfarm-launcher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("the HTTP client has a valid static configuration");
        Self { client }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, HttpError> {
        read(url, self.client.get(url)).await
    }

    pub async fn get_json_bearer<T: DeserializeOwned>(
        &self,
        url: &str,
        token: &str,
    ) -> Result<T, HttpError> {
        read(url, self.client.get(url).bearer_auth(token)).await
    }

    pub async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T, HttpError> {
        read(url, self.client.post(url).json(body)).await
    }

    pub async fn post_form<T: DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<T, HttpError> {
        read(url, self.client.post(url).form(form)).await
    }

    pub async fn post_empty<T: DeserializeOwned>(&self, url: &str) -> Result<T, HttpError> {
        read(url, self.client.post(url)).await
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

async fn read<T: DeserializeOwned>(url: &str, request: RequestBuilder) -> Result<T, HttpError> {
    let response = request
        .header("Accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(HttpError::Status {
            url: url.to_string(),
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_str(&body).map_err(|source| HttpError::Decode {
        url: url.to_string(),
        source,
    })
}
