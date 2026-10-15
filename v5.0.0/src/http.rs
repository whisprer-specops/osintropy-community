// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! A bounded HTTPS client: HTTPS only, no redirects, no retries, a 12-second timeout, at
//! most 2 MB per response, one request per second per host, and error messages that never
//! repeat a URL, token or response body.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::urlparse::{urlencode, urlsplit};

pub const USER_AGENT: &str = concat!(
    "OSINTropy/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/whisprer-specops/osintropy-community)"
);
pub const MAX_BODY: usize = 2_000_000;
pub const TIMEOUT: Duration = Duration::from_secs(12);
pub const HOST_INTERVAL: Duration = Duration::from_secs(1);

/// Why a source could not be used. Stored in the run and shown to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError(pub String);

impl SourceError {
    pub fn new(message: impl Into<String>) -> Self {
        SourceError(message.into())
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

impl From<crate::models::Invalid> for SourceError {
    fn from(err: crate::models::Invalid) -> Self {
        SourceError(err.0)
    }
}

/// Query parameters or headers, in order.
pub type Pairs<'a> = [(&'a str, String)];

/// What connectors need from an HTTP client. Tests supply recorded responses instead.
pub trait Http {
    /// GET a JSON object.
    fn get_json(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
    ) -> Result<Map<String, Value>, SourceError>;
    /// GET a JSON array.
    fn get_json_array(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
    ) -> Result<Vec<Value>, SourceError>;
    /// POST a form and read a JSON object back.
    fn post_form_json(
        &mut self,
        url: &str,
        fields: &Pairs<'_>,
    ) -> Result<Map<String, Value>, SourceError>;
}

/// A request as it goes on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// Sends a request. Returns the status and, for status 200, at most `MAX_BODY + 1` bytes of
/// body. An `Err` names the kind of network failure, with no URL in it.
pub trait Transport {
    fn send(&mut self, request: &Request) -> Result<(u16, Vec<u8>), String>;
}

/// The client connectors use for real requests.
pub struct HttpClient {
    interval: Duration,
    last: HashMap<String, Instant>,
    transport: Box<dyn Transport>,
}

impl HttpClient {
    /// A client that makes real HTTPS requests, through `HTTPS_PROXY` if it is set.
    pub fn new() -> Self {
        Self::with_transport(Box::new(UreqTransport::from_env(TIMEOUT)), HOST_INTERVAL)
    }

    /// A client over any transport, with the given gap between requests to one host.
    pub fn with_transport(transport: Box<dyn Transport>, interval: Duration) -> Self {
        HttpClient {
            interval,
            last: HashMap::new(),
            transport,
        }
    }

    fn request_json(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
        body: Option<Vec<u8>>,
    ) -> Result<Value, SourceError> {
        let parts = urlsplit(url).map_err(SourceError)?;
        let host = parts.hostname().unwrap_or_default();
        let credentials = parts.username().is_some_and(|user| !user.is_empty())
            || parts.password().is_some_and(|pass| !pass.is_empty());
        if parts.scheme != "https" || host.is_empty() || credentials || !parts.fragment.is_empty() {
            return Err(SourceError::new("connector URL must use HTTPS"));
        }
        if let Some(previous) = self.last.get(&host) {
            let elapsed = previous.elapsed();
            if elapsed < self.interval {
                thread::sleep(self.interval - elapsed);
            }
        }
        self.last.insert(host, Instant::now());
        let full_url = if params.is_empty() {
            url.to_owned()
        } else {
            let joiner = if parts.query.is_empty() { '?' } else { '&' };
            format!("{url}{joiner}{}", urlencode(params))
        };
        let mut merged: Vec<(String, String)> = vec![
            ("User-Agent".to_owned(), USER_AGENT.to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        for (name, value) in headers {
            match merged
                .iter_mut()
                .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
            {
                Some(slot) => slot.1 = value.clone(),
                None => merged.push(((*name).to_owned(), value.clone())),
            }
        }
        let request = Request {
            method: if body.is_some() { "POST" } else { "GET" },
            url: full_url,
            headers: merged,
            body,
        };
        let (status, bytes) = self
            .transport
            .send(&request)
            .map_err(|kind| SourceError(format!("network error: {kind}")))?;
        if status != 200 {
            return Err(SourceError(format!("HTTP {status}")));
        }
        if bytes.len() > MAX_BODY {
            return Err(SourceError::new("response exceeds 2 MB"));
        }
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
        serde_json::from_slice(bytes).map_err(|_| SourceError::new("invalid JSON response"))
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Http for HttpClient {
    fn get_json(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
    ) -> Result<Map<String, Value>, SourceError> {
        match self.request_json(url, params, headers, None)? {
            Value::Object(map) => Ok(map),
            _ => Err(SourceError::new("expected a JSON object")),
        }
    }

    fn get_json_array(
        &mut self,
        url: &str,
        params: &Pairs<'_>,
        headers: &Pairs<'_>,
    ) -> Result<Vec<Value>, SourceError> {
        match self.request_json(url, params, headers, None)? {
            Value::Array(items) => Ok(items),
            _ => Err(SourceError::new("expected a JSON array")),
        }
    }

    fn post_form_json(
        &mut self,
        url: &str,
        fields: &Pairs<'_>,
    ) -> Result<Map<String, Value>, SourceError> {
        let body = urlencode(fields).into_bytes();
        let headers = [(
            "Content-Type",
            "application/x-www-form-urlencoded".to_owned(),
        )];
        match self.request_json(url, &[], &headers, Some(body))? {
            Value::Object(map) => Ok(map),
            _ => Err(SourceError::new("expected a JSON object")),
        }
    }
}

/// The real network, via `ureq`, with redirects off and the proxy rules of Python's urllib:
/// `https_proxy` (or `HTTPS_PROXY`) for HTTPS, and hosts in `no_proxy` (or `NO_PROXY`)
/// reached directly. SOCKS proxies such as Tor's `socks5h://127.0.0.1:9050` work too.
pub struct UreqTransport {
    direct: ureq::Agent,
    route: Route,
    no_proxy: Option<String>,
}

/// How a request that `NO_PROXY` does not exempt leaves the machine.
enum Route {
    Direct,
    Proxy(ureq::Agent),
    /// A proxy is set but its address cannot be used. Requests fail rather than go around
    /// it, so a mistyped Tor address never sends a search out directly.
    Refused,
}

/// The error for every request a `Route::Refused` transport is asked to make.
pub const UNUSABLE_PROXY: &str = "HTTPS_PROXY holds an unsupported proxy address";

impl UreqTransport {
    pub fn from_env(timeout: Duration) -> Self {
        let lookup = |name: &str| std::env::var(name).ok();
        Self::with_proxy(
            timeout,
            proxy_setting(&lookup, "https"),
            proxy_setting(&lookup, "no"),
        )
    }

    /// A transport with an explicit proxy address and bypass list.
    pub fn with_proxy(timeout: Duration, proxy: Option<String>, no_proxy: Option<String>) -> Self {
        let builder = || {
            ureq::AgentBuilder::new()
                .timeout(timeout)
                .redirects(0)
                .user_agent(USER_AGENT)
        };
        let route = match proxy {
            None => Route::Direct,
            Some(address) => {
                match proxy_address(&address).and_then(|usable| ureq::Proxy::new(usable).ok()) {
                    Some(proxy) => Route::Proxy(builder().proxy(proxy).build()),
                    None => Route::Refused,
                }
            }
        };
        UreqTransport {
            direct: builder().build(),
            route,
            no_proxy,
        }
    }

    fn agent_for(&self, url: &str) -> Result<&ureq::Agent, String> {
        let host = urlsplit(url)
            .ok()
            .and_then(|parts| parts.hostname())
            .unwrap_or_default();
        if bypass_proxy(&host, self.no_proxy.as_deref()) {
            return Ok(&self.direct);
        }
        match &self.route {
            Route::Direct => Ok(&self.direct),
            Route::Proxy(agent) => Ok(agent),
            Route::Refused => Err(UNUSABLE_PROXY.to_owned()),
        }
    }
}

/// A proxy address in the form `ureq` reads, or `None` for one it cannot use. The scheme is
/// matched without regard to case, and an address without one is an HTTP proxy.
/// - `https` becomes `http`: Python's urllib, which 4.1.1 used, also reached such a proxy
///   over plain TCP and asked it for a tunnel.
/// - `socks5h` (SOCKS5 with the proxy resolving host names) becomes `socks5`, because
///   `ureq` already hands host names to a SOCKS5 proxy.
/// - Any other unknown scheme is refused.
fn proxy_address(address: &str) -> Option<String> {
    let Some((scheme, rest)) = address.split_once("://") else {
        return Some(address.to_owned());
    };
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => Some(format!("http://{rest}")),
        "socks5h" => Some(format!("socks5://{rest}")),
        "socks" | "socks4" | "socks4a" | "socks5" => Some(format!("{scheme}://{rest}")),
        _ => None,
    }
}

impl Transport for UreqTransport {
    fn send(&mut self, request: &Request) -> Result<(u16, Vec<u8>), String> {
        let mut call = self
            .agent_for(&request.url)?
            .request(request.method, &request.url);
        for (name, value) in &request.headers {
            call = call.set(name, value);
        }
        let outcome = match &request.body {
            Some(body) => call.send_bytes(body),
            None => call.call(),
        };
        let response = match outcome {
            Ok(response) => response,
            Err(ureq::Error::Status(code, _)) => return Ok((code, Vec::new())),
            Err(ureq::Error::Transport(transport)) => return Err(describe(&transport)),
        };
        let status = response.status();
        if status != 200 {
            return Ok((status, Vec::new()));
        }
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_BODY as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|err| io_kind(&err))?;
        Ok((status, body))
    }
}

fn io_kind(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "timeout".to_owned(),
        _ => "connection interrupted".to_owned(),
    }
}

/// A fixed description of a transport failure, so no URL or token reaches the message.
fn describe(transport: &ureq::Transport) -> String {
    use ureq::ErrorKind;
    match transport.kind() {
        ErrorKind::Dns => "DNS lookup failed".to_owned(),
        ErrorKind::ConnectionFailed => "connection failed".to_owned(),
        ErrorKind::InvalidProxyUrl | ErrorKind::ProxyConnect | ErrorKind::ProxyUnauthorized => {
            "proxy failed".to_owned()
        }
        ErrorKind::BadStatus | ErrorKind::BadHeader => "malformed response".to_owned(),
        ErrorKind::TooManyRedirects => "redirect refused".to_owned(),
        ErrorKind::Io => {
            let timed_out = std::error::Error::source(transport)
                .and_then(|source| source.downcast_ref::<std::io::Error>())
                .is_some_and(|io| {
                    matches!(
                        io.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    )
                });
            if timed_out {
                "timeout".to_owned()
            } else {
                "connection interrupted".to_owned()
            }
        }
        _ => "request failed".to_owned(),
    }
}

/// Python's `getproxies_environment()[scheme]`: any `<scheme>_proxy` variable, a
/// lower-case name winning over an upper-case one, and an empty value meaning unset.
pub fn proxy_setting(lookup: &dyn Fn(&str) -> Option<String>, scheme: &str) -> Option<String> {
    let lower = format!("{scheme}_proxy");
    match lookup(&lower) {
        Some(value) if !value.is_empty() => Some(value),
        Some(_) => None,
        None => lookup(&lower.to_uppercase()).filter(|value| !value.is_empty()),
    }
}

/// Python's `proxy_bypass_environment`: `*`, or a host equal to or ending in an entry.
pub fn bypass_proxy(host: &str, no_proxy: Option<&str>) -> bool {
    let Some(list) = no_proxy else {
        return false;
    };
    if list == "*" {
        return true;
    }
    let host = host.to_lowercase();
    list.split(',')
        .map(|name| name.trim().trim_start_matches('.').to_lowercase())
        .filter(|name| !name.is_empty())
        .any(|name| host == name || host.ends_with(&format!(".{name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Canned {
        status: u16,
        body: Vec<u8>,
        seen: Rc<RefCell<Vec<Request>>>,
        fail: Option<String>,
    }

    impl Transport for Canned {
        fn send(&mut self, request: &Request) -> Result<(u16, Vec<u8>), String> {
            self.seen.borrow_mut().push(request.clone());
            match &self.fail {
                Some(kind) => Err(kind.clone()),
                None => Ok((self.status, self.body.clone())),
            }
        }
    }

    fn client(
        status: u16,
        body: &[u8],
        fail: Option<&str>,
    ) -> (HttpClient, Rc<RefCell<Vec<Request>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let transport = Canned {
            status,
            body: body.to_vec(),
            seen: Rc::clone(&seen),
            fail: fail.map(str::to_owned),
        };
        (
            HttpClient::with_transport(Box::new(transport), Duration::ZERO),
            seen,
        )
    }

    #[test]
    fn bounds_the_body_and_requires_https() {
        let (mut http, _) = client(200, &vec![b'x'; MAX_BODY + 1], None);
        let err = http
            .get_json("https://example.org/api", &[], &[])
            .unwrap_err();
        assert!(err.0.contains("2 MB"));
        for bad in [
            "http://example.org/api",
            "https://user:pw@example.org/",
            "https://example.org/#frag",
            "https:///nohost",
        ] {
            let err = http.get_json(bad, &[], &[]).unwrap_err();
            assert!(err.0.contains("HTTPS"), "{bad}");
        }
    }

    #[test]
    fn statuses_shapes_and_secrets() {
        let (mut http, _) = client(302, b"{}", None);
        assert_eq!(
            http.get_json("https://example.org/", &[], &[])
                .unwrap_err()
                .0,
            "HTTP 302"
        );
        let (mut http, _) = client(200, b"[1]", None);
        assert_eq!(
            http.get_json("https://example.org/", &[], &[])
                .unwrap_err()
                .0,
            "expected a JSON object"
        );
        let (mut http, _) = client(200, b"{oops", None);
        assert_eq!(
            http.get_json("https://example.org/", &[], &[])
                .unwrap_err()
                .0,
            "invalid JSON response"
        );
        let (mut http, _) = client(200, b"", Some("timeout"));
        let err = http
            .get_json(
                "https://example.org/s",
                &[("api_token", "private-value".into())],
                &[],
            )
            .unwrap_err();
        assert_eq!(err.0, "network error: timeout");
        assert!(!err.0.contains("private-value"));
    }

    #[test]
    fn builds_requests_like_urllib() {
        let (mut http, seen) = client(200, b"\xef\xbb\xbf{\"access_token\":\"t\"}", None);
        let token = http
            .post_form_json(
                "https://orcid.org/oauth/token",
                &[("client_secret", "private value".into())],
            )
            .unwrap();
        assert_eq!(token["access_token"], "t");
        http.get_json(
            "https://example.org/search?x=1",
            &[
                ("q", "\"Ada Lovelace\" in:fullname".into()),
                ("n", "10".into()),
            ],
            &[
                ("Accept", "application/vnd.github+json".into()),
                ("Authorization", "Bearer s".into()),
            ],
        )
        .unwrap();
        let seen = seen.borrow();
        assert_eq!(seen[0].method, "POST");
        assert_eq!(
            seen[0].body.as_deref(),
            Some(&b"client_secret=private+value"[..])
        );
        assert_eq!(seen[1].method, "GET");
        assert_eq!(
            seen[1].url,
            "https://example.org/search?x=1&q=%22Ada+Lovelace%22+in%3Afullname&n=10"
        );
        let header = |name: &str| {
            seen[1]
                .headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(header("accept"), ["application/vnd.github+json"]);
        assert_eq!(header("user-agent"), [USER_AGENT]);
        assert_eq!(header("authorization"), ["Bearer s"]);
    }

    #[test]
    fn proxy_rules_follow_urllib() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let both = env(&[
            ("https_proxy", "socks5://127.0.0.1:9050"),
            ("HTTPS_PROXY", "http://upper:8080"),
        ]);
        assert_eq!(
            proxy_setting(&both, "https").as_deref(),
            Some("socks5://127.0.0.1:9050")
        );
        let upper = env(&[("HTTPS_PROXY", "http://upper:8080")]);
        assert_eq!(
            proxy_setting(&upper, "https").as_deref(),
            Some("http://upper:8080")
        );
        let blank = env(&[("https_proxy", ""), ("HTTPS_PROXY", "http://upper:8080")]);
        assert_eq!(proxy_setting(&blank, "https"), None);
        let http_only = env(&[("HTTP_PROXY", "http://plain:3128")]);
        assert_eq!(proxy_setting(&http_only, "https"), None);
        assert!(bypass_proxy(
            "api.github.com",
            Some("example.org, .github.com")
        ));
        assert!(bypass_proxy("github.com", Some("github.com")));
        assert!(!bypass_proxy("notgithub.com", Some("github.com")));
        assert!(bypass_proxy("anything", Some("*")));
        assert!(!bypass_proxy("anything", None));
    }

    #[test]
    fn an_unusable_proxy_refuses_instead_of_going_direct() {
        let request = Request {
            method: "GET",
            url: "https://example.org/".into(),
            headers: Vec::new(),
            body: None,
        };
        for address in [
            "ftp://proxy.example",
            "socks6://127.0.0.1:1",
            "gopher://127.0.0.1:70",
        ] {
            let mut transport =
                UreqTransport::with_proxy(Duration::from_secs(1), Some(address.into()), None);
            assert_eq!(
                transport.send(&request),
                Err(UNUSABLE_PROXY.to_owned()),
                "{address}"
            );
        }
        // Hosts in NO_PROXY still go direct, as asked.
        let transport = UreqTransport::with_proxy(
            Duration::from_secs(1),
            Some("ftp://proxy.example".into()),
            Some("example.org".into()),
        );
        assert!(transport
            .agent_for("https://api.example.org/")
            .is_ok_and(|agent| std::ptr::eq(agent, &transport.direct)));
        assert_eq!(
            proxy_address("SOCKS5H://127.0.0.1:9050").as_deref(),
            Some("socks5://127.0.0.1:9050")
        );
        assert_eq!(
            proxy_address("https://proxy.example:3128").as_deref(),
            Some("http://proxy.example:3128")
        );
        assert_eq!(
            proxy_address("127.0.0.1:8080").as_deref(),
            Some("127.0.0.1:8080")
        );
        for address in [
            "socks5h://127.0.0.1:9050",
            "http://user:pw@proxy:3128",
            "https://proxy.example:3128",
            "proxy.example:3128",
        ] {
            let transport =
                UreqTransport::with_proxy(Duration::from_secs(1), Some(address.into()), None);
            assert!(matches!(transport.route, Route::Proxy(_)), "{address}");
        }
    }

    /// The real transport against a local plain-HTTP server: it must not follow a redirect,
    /// must stop reading after the limit, and must report other statuses as they are.
    #[test]
    fn real_transport_keeps_its_bounds() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                let reply: Vec<u8> = if first.contains("/redirect") {
                    b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                } else if first.contains("/big") {
                    let body = vec![b'7'; MAX_BODY + 50];
                    let mut reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    reply.extend(body);
                    reply
                } else {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\nno"
                        .to_vec()
                };
                let _ = stream.write_all(&reply);
            }
        });
        let mut transport = UreqTransport::with_proxy(Duration::from_secs(5), None, None);
        let get = |path: &str| Request {
            method: "GET",
            url: format!("http://127.0.0.1:{port}{path}"),
            headers: vec![("User-Agent".into(), USER_AGENT.into())],
            body: None,
        };
        assert_eq!(transport.send(&get("/redirect")).unwrap().0, 302);
        let (status, body) = transport.send(&get("/big")).unwrap();
        assert_eq!((status, body.len()), (200, MAX_BODY + 1));
        assert_eq!(transport.send(&get("/missing")).unwrap().0, 404);
        server.join().unwrap();
    }
}
