use reqwest::{StatusCode, Url};
use serde::Deserialize;
use std::{
    io::ErrorKind,
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig, stream::MaybeTlsStream};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const AUTH_TIMEOUT: Duration = Duration::from_secs(180);
const SSO_HOST: &str = "sso.nexusmods.com";
const KEYRING_SERVICE: &str = "com.chipichapa.raesir.nexus";

fn app_slug() -> Option<&'static str> {
    option_env!("RAESIR_NEXUS_APP_SLUG").filter(|slug| {
        !slug.is_empty()
            && slug.len() <= 80
            && slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

pub fn configured() -> bool {
    app_slug().is_some()
}

pub fn testing() -> bool {
    cfg!(debug_assertions) && std::env::var_os("RAESIR_NEXUS_TEST_API_KEY_FILE").is_some()
}

fn credential() -> Result<keyring::Entry, String> {
    let slug = app_slug().ok_or("Raesir needs a registered Nexus Mods application slug")?;
    keyring::Entry::new(KEYRING_SERVICE, slug).map_err(|_| keyring_error())
}

fn keyring_error() -> String {
    "Could not access the system credential store for Nexus Mods".into()
}

pub fn current() -> Result<Option<String>, String> {
    if testing() && !configured() {
        return validate(&api_key()?)?
            .map(Some)
            .ok_or_else(|| "Nexus Mods rejected the test API key".into());
    }
    let entry = credential()?;
    let key = match entry.get_password() {
        Ok(key) => key,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(_) => return Err(keyring_error()),
    };
    if let Some(name) = validate(&key)? {
        return Ok(Some(name));
    }
    sign_out()?;
    Ok(None)
}

pub(super) fn api_key() -> Result<String, String> {
    if !configured() && testing() {
        use std::io::Read;
        let path = std::env::var_os("RAESIR_NEXUS_TEST_API_KEY_FILE")
            .ok_or("No Nexus test key file is configured")?;
        let mut key = String::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(4097).read_to_string(&mut key))
            .map_err(|_| "Could not read the Nexus test key file")?;
        let key = key.trim();
        if key.is_empty() || key.len() > 4096 || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("The Nexus test key file must contain a single API key".into());
        }
        return Ok(key.to_owned());
    }
    credential()?.get_password().map_err(|error| match error {
        keyring::Error::NoEntry => "Sign in to Nexus Mods first".into(),
        _ => keyring_error(),
    })
}

pub fn sign_out() -> Result<(), String> {
    match credential()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(keyring_error()),
    }
}

fn validate(key: &str) -> Result<Option<String>, String> {
    let client = crate::http::credentials()
        .map_err(|_| "Could not prepare Nexus Mods account validation")?;
    let response = client
        .get("https://api.nexusmods.com/v1/users/validate.json")
        .timeout(HANDSHAKE_TIMEOUT)
        .header("Application-Name", "Raesir")
        .header("Application-Version", env!("CARGO_PKG_VERSION"))
        .header("apikey", key)
        .send()
        .map_err(|_| "Could not reach Nexus Mods to verify your account")?;
    match response.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Ok(None),
        status if status.is_success() => {}
        _ => return Err("Nexus Mods could not verify your account. Try again.".into()),
    }
    #[derive(Deserialize)]
    struct Account {
        name: String,
    }
    response
        .json::<Account>()
        .map(|account| Some(account.name))
        .map_err(|_| "Nexus Mods returned an invalid account response".into())
}

pub struct SignIn {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    url: Url,
}

impl SignIn {
    pub fn start() -> Result<Self, String> {
        let slug = app_slug().ok_or("Raesir needs a registered Nexus Mods application slug")?;
        let stream = (SSO_HOST, 443)
            .to_socket_addrs()
            .map_err(|_| "Could not find the Nexus Mods sign-in service")?
            .find_map(|address| TcpStream::connect_timeout(&address, HANDSHAKE_TIMEOUT).ok())
            .ok_or("Could not connect to the Nexus Mods sign-in service")?;
        stream
            .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
            .map_err(|_| connection_error())?;
        stream
            .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
            .map_err(|_| connection_error())?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(16 * 1024))
            .max_frame_size(Some(16 * 1024));
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| connection_error())?
        .with_root_certificates(rustls::RootCertStore::from_iter(
            webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
        ))
        .with_no_client_auth();
        let (mut socket, _) = tungstenite::client_tls_with_config(
            "wss://sso.nexusmods.com/",
            stream,
            Some(config),
            Some(tungstenite::Connector::Rustls(Arc::new(tls))),
        )
        .map_err(|_| connection_error())?;
        let id = uuid::Uuid::new_v4().to_string();
        let request = serde_json::json!({"id": &id, "token": null, "protocol": 2});
        socket
            .send(Message::Text(request.to_string().into()))
            .map_err(|_| connection_error())?;
        if read_sso(&mut socket, HANDSHAKE_TIMEOUT, || false)? != SsoEvent::Ready {
            return Err("Nexus Mods did not start a sign-in session".into());
        }
        let mut url = Url::parse("https://www.nexusmods.com/sso").expect("valid Nexus sign-in URL");
        url.query_pairs_mut()
            .append_pair("id", &id)
            .append_pair("application", slug);
        Ok(Self { socket, url })
    }

    pub fn url(&self) -> &str {
        self.url.as_str()
    }

    pub fn finish(mut self, cancelled: impl Fn() -> bool) -> Result<String, String> {
        let SsoEvent::Granted(key) = read_sso(&mut self.socket, AUTH_TIMEOUT, &cancelled)? else {
            return Err("Nexus Mods did not authorize Raesir".into());
        };
        let name = validate(&key)?.ok_or("Nexus Mods rejected the sign-in credentials")?;
        if cancelled() {
            return Err("Nexus Mods sign-in was cancelled".into());
        }
        credential()?
            .set_password(&key)
            .map_err(|_| keyring_error())?;
        Ok(name)
    }
}

#[derive(PartialEq, Eq)]
enum SsoEvent {
    Ready,
    Granted(String),
}

fn parse_sso(text: &str) -> Result<SsoEvent, String> {
    let response: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| "Nexus Mods returned an invalid sign-in response")?;
    if response["success"].as_bool() != Some(true) {
        return Err("Nexus Mods declined sign-in. Try again.".into());
    }
    let data = &response["data"];
    if let Some(key) = data["api_key"].as_str().filter(|key| !key.is_empty()) {
        return Ok(SsoEvent::Granted(key.into()));
    }
    if data["connection_token"]
        .as_str()
        .is_some_and(|token| !token.is_empty())
    {
        return Ok(SsoEvent::Ready);
    }
    Err("Nexus Mods returned an incomplete sign-in response".into())
}

fn connection_error() -> String {
    "The Nexus Mods sign-in connection closed. Try again.".into()
}

fn read_sso(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> Result<SsoEvent, String> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Rustls(stream) => &mut stream.sock,
        _ => return Err(connection_error()),
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|_| connection_error())?;
    let end = Instant::now() + timeout;
    let mut keepalive = Instant::now() + Duration::from_secs(20);
    while !cancelled() && Instant::now() < end {
        if Instant::now() >= keepalive {
            socket
                .send(Message::Ping(Vec::new().into()))
                .map_err(|_| connection_error())?;
            keepalive = Instant::now() + Duration::from_secs(20);
        }
        match socket.read() {
            Ok(Message::Text(text)) => return parse_sso(&text),
            Ok(Message::Close(_)) => return Err(connection_error()),
            Ok(_) => socket.flush().map_err(|_| connection_error())?,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err(connection_error()),
        }
    }
    Err(if cancelled() {
        "Nexus Mods sign-in was cancelled"
    } else {
        "Nexus Mods sign-in timed out. Try again."
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_authorization_grant_contains_an_api_key() {
        assert!(matches!(
            parse_sso(r#"{"success":true,"data":{"connection_token":"ready"}}"#),
            Ok(SsoEvent::Ready)
        ));
        assert!(
            matches!(parse_sso(r#"{"success":true,"data":{"api_key":"fixture"}}"#), Ok(SsoEvent::Granted(key)) if key == "fixture")
        );
        let error =
            parse_sso(r#"{"success":false,"error":"private detail","data":{"api_key":"fixture"}}"#);
        assert!(
            matches!(error, Err(error) if !error.contains("fixture") && !error.contains("private detail"))
        );
    }
}
