use serde_json::{Value, json};
use std::{
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};

const LIMIT: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(6);
const UNAVAILABLE: &str = "The client's download interface is unavailable";

/// Only connect after verifying that this loopback listener belongs to the
/// selected client. Redirects, credentials, arbitrary hosts and paths are denied.
pub(super) struct Socket {
    socket: WebSocket<TcpStream>,
    id: u64,
}

impl Socket {
    pub(super) fn connect(address: &str, port: u16) -> Result<Self, String> {
        let url = reqwest::Url::parse(address).map_err(|_| UNAVAILABLE)?;
        if url.scheme() != "ws"
            || url.host_str() != Some("127.0.0.1")
            || url.port() != Some(port)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.path() == "/" || url.path().starts_with("/devtools/page/"))
        {
            return Err(UNAVAILABLE.into());
        }
        let stream = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            Duration::from_secs(1),
        )
        .map_err(|_| UNAVAILABLE)?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|_| UNAVAILABLE)?;
        stream
            .set_write_timeout(Some(TIMEOUT))
            .map_err(|_| UNAVAILABLE)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(LIMIT))
            .max_frame_size(Some(LIMIT));
        let (socket, _) = tungstenite::client::client_with_config(address, stream, Some(config))
            .map_err(|_| UNAVAILABLE)?;
        Ok(Self { socket, id: 0 })
    }

    pub(super) fn next_id(&mut self) -> u64 {
        self.id += 1;
        self.id
    }

    pub(super) fn send(&mut self, value: &Value) -> Result<(), String> {
        let text = serde_json::to_string(value).map_err(|_| UNAVAILABLE)?;
        if text.len() > LIMIT {
            return Err(UNAVAILABLE.into());
        }
        self.socket
            .send(Message::Text(text.into()))
            .map_err(|_| UNAVAILABLE.into())
    }

    pub(super) fn receive(&mut self, deadline: Instant) -> Result<Value, String> {
        while Instant::now() < deadline {
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1));
            self.socket
                .get_mut()
                .set_read_timeout(Some(remaining))
                .map_err(|_| UNAVAILABLE)?;
            match self.socket.read().map_err(|_| UNAVAILABLE)? {
                Message::Text(text) => {
                    return serde_json::from_str(&text).map_err(|_| UNAVAILABLE.into());
                }
                Message::Ping(_) => self.socket.flush().map_err(|_| UNAVAILABLE)?,
                Message::Pong(_) => {}
                _ => return Err(UNAVAILABLE.into()),
            }
        }
        Err(UNAVAILABLE.into())
    }

    pub(super) fn evaluate(&mut self, script: &str) -> Result<Value, String> {
        let id = self.next_id();
        self.send(&json!({"id":id,"method":"Runtime.evaluate","params":{
            "expression":script,"returnByValue":true,"awaitPromise":true
        }}))?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let value = self.receive(deadline)?;
            if value["id"].as_u64() != Some(id) {
                continue;
            }
            if value.get("error").is_some() || value["result"].get("exceptionDetails").is_some() {
                return Err(
                    "The client could not complete this download request. Check that it is signed in and its library has loaded".into(),
                );
            }
            return value
                .pointer("/result/result/value")
                .cloned()
                .ok_or_else(|| UNAVAILABLE.into());
        }
    }
}

pub(super) fn cdp(port: u16, allowed: impl Fn(&str) -> bool) -> Result<Socket, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| UNAVAILABLE)?;
    let mut response = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|_| UNAVAILABLE)?;
    let mut bytes = Vec::new();
    use std::io::Read;
    response
        .by_ref()
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UNAVAILABLE)?;
    if bytes.len() > LIMIT {
        return Err(UNAVAILABLE.into());
    }
    let pages: Value = serde_json::from_slice(&bytes).map_err(|_| UNAVAILABLE)?;
    for page in pages.as_array().ok_or(UNAVAILABLE)? {
        if page["type"] == "page"
            && page["url"].as_str().is_some_and(&allowed)
            && let Some(address) = page["webSocketDebuggerUrl"].as_str()
        {
            return Socket::connect(address, port);
        }
    }
    Err(UNAVAILABLE.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    #[test]
    fn cdp_ignores_events_and_never_accepts_a_javascript_exception_as_success() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut peer = tungstenite::accept(stream).unwrap();
            let request: Value =
                serde_json::from_str(peer.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "Runtime.evaluate");
            peer.send(Message::Text(
                json!({"method":"Runtime.consoleAPICalled","params":{}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
            peer.send(Message::Text(json!({"id":request["id"],"result":{"result":{"type":"object","value":{"downloads":[]}}}}).to_string().into())).unwrap();
            let request: Value =
                serde_json::from_str(peer.read().unwrap().to_text().unwrap()).unwrap();
            peer.send(Message::Text(
                json!({"id":request["id"],"result":{"exceptionDetails":{"text":"Error"}}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        });
        let mut peer = Socket::connect(
            &format!("ws://127.0.0.1:{port}/devtools/page/fixture"),
            port,
        )
        .unwrap();
        assert_eq!(
            peer.evaluate("fixture read").unwrap(),
            json!({"downloads":[]})
        );
        assert!(peer.evaluate("fixture unavailable operation").is_err());
        server.join().unwrap();
    }

    #[test]
    fn client_supplied_debugger_addresses_cannot_redirect_the_bridge() {
        for url in [
            "wss://127.0.0.1:9222/devtools/page/x",
            "ws://localhost:9222/devtools/page/x",
            "ws://192.0.2.1:9222/devtools/page/x",
            "ws://127.0.0.1:9223/devtools/page/x",
            "ws://user@127.0.0.1:9222/devtools/page/x",
            "ws://127.0.0.1:9222/other",
            "ws://127.0.0.1:9222/devtools/page/x?redirect=1",
        ] {
            assert!(Socket::connect(url, 9222).is_err());
        }
    }
}
