//! Shared blocking HTTP clients.
//!
//! Building a blocking reqwest client starts a runtime thread and loads TLS
//! roots, so clients are built once and reused for connection pooling. Callers
//! that need a different total timeout set it per request with `.timeout()`.
use reqwest::blocking::{Client, ClientBuilder};
use std::{sync::LazyLock, time::Duration};

pub const USER_AGENT: &str = concat!("Raesir/", env!("CARGO_PKG_VERSION"));
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(30);

pub type Shared = LazyLock<Result<Client, String>>;

/// Applies the shared user agent and connect timeout. Prefer the shared clients
/// below; build one only when helpers it is passed to need its total `timeout`.
pub fn build(builder: ClientBuilder, timeout: Duration) -> Result<Client, String> {
    builder
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())
}

pub fn shared(client: &'static Shared) -> Result<&'static Client, String> {
    client.as_ref().map_err(Clone::clone)
}

/// General-purpose client that follows redirects.
pub fn client() -> Result<&'static Client, String> {
    static CLIENT: Shared = LazyLock::new(|| build(Client::builder(), TIMEOUT));
    shared(&CLIENT)
}

/// Client for fixed HTTPS endpoints; refuses plain HTTP, including redirects.
pub fn https_only() -> Result<&'static Client, String> {
    static CLIENT: Shared = LazyLock::new(|| build(Client::builder().https_only(true), TIMEOUT));
    shared(&CLIENT)
}

/// HTTPS client that never follows redirects, for requests carrying credentials.
pub fn credentials() -> Result<&'static Client, String> {
    static CLIENT: Shared = LazyLock::new(|| {
        build(
            Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none()),
            TIMEOUT,
        )
    });
    shared(&CLIENT)
}

/// Tests and offline runs set `RAESIR_DISABLE_NETWORK` to forbid requests.
pub fn network_disabled() -> bool {
    std::env::var_os("RAESIR_DISABLE_NETWORK").is_some()
}
