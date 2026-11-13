//! A deliberately small HTTPS client for the update check.
//!
//! TLS is rustls (pure Rust) with the Mozilla root store compiled in, so no
//! Windows networking or certificate DLLs (winhttp, schannel, crypt32...) are
//! loaded; only the socket layer the standard library already links.
//!
//! Scope: HTTPS GET, HTTP/1.1, following HTTPS-only redirects, Content-Length
//! and chunked bodies, optional HTTP CONNECT proxy with basic auth. Every
//! read is bounded in size and time.
//!
//! Proxies, in order of precedence: `Proxy=` in policy.ini (`direct` turns
//! proxies off), the HTTPS_PROXY environment variable (with NO_PROXY), then
//! the user's manual proxy setting from Windows' Internet Options, as the
//! Python version used. Automatic configuration scripts (PAC/WPAD) and proxies
//! that require Windows sign-in (NTLM/Kerberos) are not supported. If the
//! network re-signs TLS traffic, point `ExtraRootCertificates=` at the
//! inspection CA in PEM form.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use super::fsutil;

const MAX_HEADER_LINE: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADERS: usize = 100;
const MAX_REDIRECTS: usize = 5;
const MAX_CERT_FILE: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Options {
    pub user_agent: String,
    /// Explicit proxy (`http://[user:pass@]host:port`). `None` falls back to
    /// the HTTPS_PROXY environment variable, honouring NO_PROXY.
    pub proxy: Option<String>,
    /// PEM file of additional trusted root certificates.
    pub extra_root_certificates: Option<PathBuf>,
    pub connect_timeout: Duration,
    /// Longest wait for any single read or write.
    pub io_timeout: Duration,
    /// Hard limit for the whole request, including the body.
    pub total_timeout: Duration,
}

#[derive(Debug)]
pub enum HttpError {
    InvalidUrl,
    NotHttps,
    InvalidProxy,
    Certificates(String),
    Connect(io::Error),
    ProxyRefused(u16),
    Tls(String),
    Io(io::Error),
    TimedOut,
    Protocol(&'static str),
    Status(u16),
    TooLarge,
    TooManyRedirects,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::InvalidUrl => f.write_str("the address is not valid"),
            HttpError::NotHttps => f.write_str("refused to use an unencrypted (non-HTTPS) address"),
            HttpError::InvalidProxy => f.write_str("the proxy setting is not valid (expected http://host:port)"),
            HttpError::Certificates(why) => write!(f, "could not load the extra root certificates: {why}"),
            HttpError::Connect(e) => write!(f, "could not connect: {e}"),
            HttpError::ProxyRefused(code) => write!(f, "the proxy refused the connection (HTTP {code})"),
            HttpError::Tls(why) => write!(f, "secure connection failed: {why}"),
            HttpError::Io(e) => write!(f, "network error: {e}"),
            HttpError::TimedOut => f.write_str("the server took too long to respond"),
            HttpError::Protocol(why) => write!(f, "unexpected response from the server ({why})"),
            HttpError::Status(code) => write!(f, "the server answered HTTP {code}"),
            HttpError::TooLarge => f.write_str("the download is larger than allowed"),
            HttpError::TooManyRedirects => f.write_str("too many redirects"),
        }
    }
}

impl std::error::Error for HttpError {}

fn io_err(e: io::Error) -> HttpError {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HttpError::TimedOut,
        io::ErrorKind::InvalidData => match e.get_ref() {
            // rustls reports certificate and handshake failures this way.
            Some(inner) => HttpError::Tls(inner.to_string()),
            None => HttpError::Io(e),
        },
        _ => HttpError::Io(e),
    }
}

// ---------------------------------------------------------------- URLs

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Path and query, always starting with '/'.
    pub path: String,
}

impl Url {
    /// Parse an absolute `https://` URL. Anything else is refused.
    pub fn parse(text: &str) -> Result<Url, HttpError> {
        let rest = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
            Some((scheme, _)) if scheme.eq_ignore_ascii_case("http") => return Err(HttpError::NotHttps),
            _ => return Err(HttpError::InvalidUrl),
        };
        let split = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(split);
        let (host, port) = split_host_port(authority, 443).ok_or(HttpError::InvalidUrl)?;
        let tail = tail.split('#').next().unwrap_or("");
        let path = if tail.starts_with('/') { tail.to_string() } else { format!("/{tail}") };
        if path.bytes().any(|b| b <= b' ' || b >= 0x7f) {
            return Err(HttpError::InvalidUrl);
        }
        Ok(Url { host, port, path })
    }

    /// Host as it appears in a URL or header: IPv6 addresses in brackets.
    fn host_literal(&self) -> String {
        if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() }
    }

    fn host_header(&self) -> String {
        if self.port == 443 { self.host_literal() } else { format!("{}:{}", self.host_literal(), self.port) }
    }

    /// Resolve a redirect target against this URL. HTTPS only.
    fn join(&self, location: &str) -> Result<Url, HttpError> {
        let location = location.trim();
        let has_scheme = location.split_once(':').is_some_and(|(scheme, _)| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        });
        if has_scheme {
            Url::parse(location)
        } else if let Some(rest) = location.strip_prefix("//") {
            Url::parse(&format!("https://{rest}"))
        } else if location.starts_with('/') {
            Url::parse(&format!("https://{}{}", self.host_header(), location))
        } else {
            Err(HttpError::InvalidUrl)
        }
    }
}

/// Split "host[:port]" or "[v6]:port". No user-info allowed.
fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        if !host.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') {
            return None;
        }
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p),
            None if after.is_empty() => None,
            None => return None,
        };
        (host.to_string(), port)
    } else {
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
            return None;
        }
        (host.to_ascii_lowercase(), port)
    };
    let port = match port {
        None => default_port,
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
            p.parse::<u16>().ok().filter(|&n| n != 0)?
        }
        Some(_) => return None,
    };
    Some((host, port))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Proxy {
    host: String,
    port: u16,
    /// Pre-encoded "Basic ..." credentials.
    authorization: Option<String>,
}

impl Proxy {
    fn parse(text: &str) -> Result<Proxy, HttpError> {
        let text = text.trim();
        let rest = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
            Some(_) => return Err(HttpError::InvalidProxy),
            None => text,
        };
        let rest = rest.trim_end_matches('/');
        let (userinfo, hostport) = match rest.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, rest),
        };
        let (host, port) = split_host_port(hostport, 80).ok_or(HttpError::InvalidProxy)?;
        let authorization = userinfo.map(|u| format!("Basic {}", base64(percent_decode(u).as_bytes())));
        Ok(Proxy { host, port, authorization })
    }

    /// "direct" (or "none") in policy.ini means: never use a proxy.
    fn is_direct(text: &str) -> bool {
        matches!(text.trim().to_ascii_lowercase().as_str(), "direct" | "none")
    }

    /// HTTPS_PROXY from the environment, unless NO_PROXY covers `host`.
    fn from_env(host: &str) -> Option<Result<Proxy, HttpError>> {
        let value = ["HTTPS_PROXY", "https_proxy"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))?;
        let no_proxy = ["NO_PROXY", "no_proxy"].iter().find_map(|name| std::env::var(name).ok()).unwrap_or_default();
        if no_proxy_matches(&no_proxy, host) {
            return None;
        }
        Some(Proxy::parse(&value))
    }
}

/// Windows' manual proxy (Internet Options > Connections > LAN settings),
/// read the way Python's urllib did. `ProxyOverride` is honoured.
fn system_proxy(host: &str) -> Option<Result<Proxy, HttpError>> {
    let settings = winproxy::read()?;
    if settings.bypass(host) {
        return None;
    }
    Some(Proxy::parse(&settings.server_for_https()?))
}

#[derive(Debug, Default)]
struct SystemProxySettings {
    server: String,
    overrides: String,
}

impl SystemProxySettings {
    /// "host:port", or "http=h:p;https=h:p;..." per protocol.
    fn server_for_https(&self) -> Option<String> {
        let server = self.server.trim();
        if !server.contains('=') {
            return (!server.is_empty()).then(|| strip_scheme(server));
        }
        let entries: Vec<(String, &str)> = server
            .split(';')
            .filter_map(|e| e.split_once('='))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
            .collect();
        ["https", "http"]
            .iter()
            .find_map(|want| entries.iter().find(|(k, v)| k == want && !v.is_empty()).map(|(_, v)| strip_scheme(v)))
    }

    fn bypass(&self, host: &str) -> bool {
        self.overrides.split(';').map(str::trim).filter(|p| !p.is_empty()).any(|pattern| {
            if pattern.eq_ignore_ascii_case("<local>") {
                !host.contains('.')
            } else {
                glob_match(&pattern.to_ascii_lowercase(), host)
            }
        })
    }
}

fn strip_scheme(address: &str) -> String {
    match address.split_once("://") {
        Some((_, rest)) => rest.to_string(),
        None => address.to_string(),
    }
}

/// Case-sensitive glob with `*` (callers lower-case both sides).
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

#[cfg(windows)]
mod winproxy {
    use super::SystemProxySettings;
    use crate::common::winreg::{Key, Root, View};

    pub(super) fn read() -> Option<SystemProxySettings> {
        let key = Key::open(
            Root::CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            View::Native64,
        )?;
        if key.dword("ProxyEnable")? == 0 {
            return None;
        }
        Some(SystemProxySettings {
            server: key.string("ProxyServer").unwrap_or_default(),
            overrides: key.string("ProxyOverride").unwrap_or_default(),
        })
    }
}

#[cfg(not(windows))]
mod winproxy {
    pub(super) fn read() -> Option<super::SystemProxySettings> {
        None
    }
}

fn no_proxy_matches(list: &str, host: &str) -> bool {
    list.split(',')
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .any(|entry| !entry.is_empty() && (entry == "*" || host == entry || host.ends_with(&format!(".{entry}"))))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16);
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------- client

pub struct Client {
    options: Options,
    tls: Arc<ClientConfig>,
}

impl Client {
    pub fn new(options: Options) -> Result<Client, HttpError> {
        let tls = Arc::new(tls_config(options.extra_root_certificates.as_deref())?);
        if let Some(p) = options.proxy.as_deref().filter(|p| !Proxy::is_direct(p)) {
            Proxy::parse(p)?; // report a bad setting now, not mid-request
        }
        Ok(Client { options, tls })
    }

    /// GET `url`, following up to five HTTPS redirects. Only a final 200 is
    /// accepted. `max_body` caps the response size.
    pub fn get(&self, url: &str, max_body: u64) -> Result<Response, HttpError> {
        let deadline = Instant::now() + self.options.total_timeout;
        let mut url = Url::parse(url)?;
        for _ in 0..=MAX_REDIRECTS {
            let mut conn = self.connect(&url, deadline)?;
            let request = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
                url.path,
                url.host_header(),
                self.options.user_agent
            );
            conn.write_all(request.as_bytes())?;
            let head = read_head(&mut conn)?;
            match head.status {
                200 => return Response::new(conn, &head, max_body),
                301 | 302 | 303 | 307 | 308 => {
                    let location = head.header("location").ok_or(HttpError::Protocol("redirect without a location"))?;
                    url = url.join(location)?;
                }
                code => return Err(HttpError::Status(code)),
            }
        }
        Err(HttpError::TooManyRedirects)
    }

    fn proxy_for(&self, host: &str) -> Result<Option<Proxy>, HttpError> {
        match &self.options.proxy {
            Some(p) if Proxy::is_direct(p) => Ok(None),
            Some(p) => Proxy::parse(p).map(Some),
            None => Proxy::from_env(host).or_else(|| system_proxy(host)).transpose(),
        }
    }

    fn connect(&self, url: &Url, deadline: Instant) -> Result<Conn, HttpError> {
        let proxy = self.proxy_for(&url.host)?;
        let (host, port) = match &proxy {
            Some(p) => (p.host.as_str(), p.port),
            None => (url.host.as_str(), url.port),
        };

        let addrs = resolve(host, port, remaining(deadline)?.min(self.options.connect_timeout))?;
        let tcp = connect_any(addrs, deadline, self.options.connect_timeout)?;
        let _ = tcp.set_nodelay(true);
        let mut sock = TimedSocket { tcp, deadline, io_timeout: self.options.io_timeout };

        if let Some(proxy) = &proxy {
            tunnel(&mut sock, proxy, url, &self.options.user_agent)?;
        }

        let name = ServerName::try_from(url.host.clone()).map_err(|_| HttpError::InvalidUrl)?;
        let mut tls = ClientConnection::new(self.tls.clone(), name).map_err(|e| HttpError::Tls(e.to_string()))?;
        // Handshake now, so its reads and writes are bounded by the deadline
        // like everything else.
        while tls.is_handshaking() {
            tls.complete_io(&mut sock).map_err(io_err)?;
        }
        let stream = StreamOwned::new(tls, sock);
        Ok(Conn::new(Box::new(stream), deadline))
    }
}

/// How long to give one address before also trying the next (RFC 8305,
/// "Happy Eyeballs").
const ATTEMPT_DELAY: Duration = Duration::from_millis(250);
/// At most this many addresses are tried.
const MAX_ATTEMPTS: usize = 8;

/// Connect to whichever address answers first. Attempts start 250 ms apart,
/// alternating IPv6 and IPv4, and run side by side, the way browsers do it.
/// Trying one address after another instead lets a network with broken IPv6
/// (common, and invisible in a browser) eat the whole time limit on
/// addresses that never answer.
fn connect_any(addrs: Vec<SocketAddr>, deadline: Instant, connect_timeout: Duration) -> Result<TcpStream, HttpError> {
    let (v6, v4): (Vec<SocketAddr>, Vec<SocketAddr>) = addrs.into_iter().partition(SocketAddr::is_ipv6);
    let (first, second) = match v6.first() {
        // Follow the resolver's preference for which family goes first.
        Some(_) if !v4.is_empty() && !v6.is_empty() => (v6, v4),
        _ => (v4, v6),
    };
    let mut ordered = Vec::new();
    let (mut a, mut b) = (first.into_iter(), second.into_iter());
    loop {
        match (a.next(), b.next()) {
            (None, None) => break,
            (x, y) => ordered.extend(x.into_iter().chain(y)),
        }
    }
    ordered.truncate(MAX_ATTEMPTS);
    if ordered.is_empty() {
        return Err(HttpError::Connect(io::Error::new(io::ErrorKind::NotFound, "no addresses found")));
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let mut started = 0;
    let mut finished = 0;
    loop {
        if started < ordered.len() {
            let addr = ordered[started];
            let timeout = remaining(deadline)?.min(connect_timeout);
            let tx = tx.clone();
            // A losing attempt ends on its own within `timeout`; its socket
            // is dropped when the send below finds nobody listening.
            std::thread::Builder::new()
                .name("connect".into())
                .spawn(move || {
                    let _ = tx.send(TcpStream::connect_timeout(&addr, timeout));
                })
                .map_err(HttpError::Connect)?;
            started += 1;
        }
        let wait = if started < ordered.len() { ATTEMPT_DELAY } else { remaining(deadline)? };
        match rx.recv_timeout(wait.min(remaining(deadline)?)) {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => {
                finished += 1;
                if finished == ordered.len() {
                    // Every address failed; report the last reason.
                    return Err(match e.kind() {
                        io::ErrorKind::TimedOut => HttpError::TimedOut,
                        _ => HttpError::Connect(e),
                    });
                }
                // A failure frees the next attempt to start straight away.
                if started < ordered.len() {
                    continue;
                }
            }
            Err(_) => {}
        }
        remaining(deadline)?;
    }
}

/// Name lookup with a time limit. The system resolver has none of its own,
/// so it runs on a helper thread that is abandoned if it takes too long.
fn resolve(host: &str, port: u16, timeout: Duration) -> Result<Vec<SocketAddr>, HttpError> {
    let (tx, rx) = std::sync::mpsc::channel();
    let target = (host.to_string(), port);
    std::thread::Builder::new()
        .name("dns".into())
        .spawn(move || {
            let _ = tx.send(target.to_socket_addrs().map(Iterator::collect::<Vec<_>>));
        })
        .map_err(HttpError::Connect)?;
    match rx.recv_timeout(timeout) {
        Ok(Ok(addrs)) => Ok(addrs),
        Ok(Err(e)) => Err(HttpError::Connect(e)),
        Err(_) => Err(HttpError::TimedOut),
    }
}

/// A socket that applies the request deadline to every read and write:
/// each call waits at most `min(io_timeout, time left)`, and fails once the
/// deadline has passed, so a server trickling bytes cannot stretch a request
/// past its limit.
struct TimedSocket {
    tcp: TcpStream,
    deadline: Instant,
    io_timeout: Duration,
}

impl TimedSocket {
    fn budget(&self) -> io::Result<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request deadline passed"));
        }
        Ok(left.min(self.io_timeout))
    }
}

impl Read for TimedSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.tcp.set_read_timeout(Some(self.budget()?))?;
        self.tcp.read(buf)
    }
}

impl Write for TimedSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tcp.set_write_timeout(Some(self.budget()?))?;
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

fn remaining(deadline: Instant) -> Result<Duration, HttpError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() { Err(HttpError::TimedOut) } else { Ok(left) }
}

fn tls_config(extra_roots: Option<&Path>) -> Result<ClientConfig, HttpError> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = extra_roots {
        let pem = fsutil::read_capped(path, MAX_CERT_FILE)
            .map_err(|e| HttpError::Certificates(format!("{}: {e}", path.display())))?;
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&pem)
            .collect::<Result<_, _>>()
            .map_err(|e| HttpError::Certificates(format!("{}: {e:?}", path.display())))?;
        let (added, _ignored) = roots.add_parsable_certificates(certs);
        if added == 0 {
            return Err(HttpError::Certificates(format!("{}: no usable certificates", path.display())));
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| HttpError::Tls(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// Ask an HTTP proxy for a raw tunnel to the origin (HTTP CONNECT).
fn tunnel(sock: &mut TimedSocket, proxy: &Proxy, url: &Url, user_agent: &str) -> Result<(), HttpError> {
    let target = format!("{}:{}", url.host_literal(), url.port);
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nUser-Agent: {user_agent}\r\n");
    if let Some(auth) = &proxy.authorization {
        request.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
    }
    request.push_str("\r\n");
    sock.write_all(request.as_bytes()).map_err(io_err)?;

    // Read the proxy's reply one byte at a time so nothing belonging to the
    // TLS handshake is consumed. It is a few hundred bytes at most.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") {
        if head.len() >= MAX_HEADER_BYTES {
            return Err(HttpError::Protocol("proxy response too long"));
        }
        match sock.read(&mut byte).map_err(io_err)? {
            0 => return Err(HttpError::Protocol("proxy closed the connection")),
            _ => head.push(byte[0]),
        }
    }
    let status = parse_status_line(head.split(|&b| b == b'\n').next().unwrap_or(&[]))?;
    if !(200..300).contains(&status) {
        return Err(HttpError::ProxyRefused(status));
    }
    Ok(())
}

// ------------------------------------------------------- buffered transport

/// What the response reader needs from the connection. Abstracted so the
/// framing code can be tested without a network. Time limits are enforced
/// underneath, by `TimedSocket`.
pub(crate) trait Transport: Read + Write + Send {}

impl<T: Read + Write + Send> Transport for T {}

pub(crate) struct Conn {
    stream: Box<dyn Transport>,
    buf: Vec<u8>,
    start: usize,
    end: usize,
    deadline: Instant,
}

impl Conn {
    pub(crate) fn new(stream: Box<dyn Transport>, deadline: Instant) -> Conn {
        Conn { stream, buf: vec![0; 16 * 1024], start: 0, end: 0, deadline }
    }

    fn write_all(&mut self, data: &[u8]) -> Result<(), HttpError> {
        remaining(self.deadline)?;
        self.stream.write_all(data).map_err(io_err)?;
        self.stream.flush().map_err(io_err)
    }

    /// Refill the buffer if it is empty. Returns bytes available (0 = EOF).
    fn fill(&mut self) -> Result<usize, HttpError> {
        if self.start < self.end {
            return Ok(self.end - self.start);
        }
        remaining(self.deadline)?;
        let n = loop {
            match self.stream.read(&mut self.buf) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err(e)),
            }
        };
        self.start = 0;
        self.end = n;
        Ok(n)
    }

    /// One line without its CR LF. Errors past `max` bytes or on EOF.
    fn read_line(&mut self, max: usize) -> Result<Vec<u8>, HttpError> {
        let mut line = Vec::new();
        loop {
            if self.fill()? == 0 {
                return Err(HttpError::Protocol("connection closed early"));
            }
            let available = &self.buf[self.start..self.end];
            if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                line.extend_from_slice(&available[..pos]);
                self.start += pos + 1;
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.len() > max {
                    return Err(HttpError::Protocol("header line too long"));
                }
                return Ok(line);
            }
            line.extend_from_slice(available);
            self.start = self.end;
            if line.len() > max {
                return Err(HttpError::Protocol("header line too long"));
            }
        }
    }

    /// Up to `out.len()` bytes. 0 means the peer closed the connection.
    fn read_some(&mut self, out: &mut [u8]) -> Result<usize, HttpError> {
        let available = self.fill()?;
        let n = available.min(out.len());
        out[..n].copy_from_slice(&self.buf[self.start..self.start + n]);
        self.start += n;
        Ok(n)
    }
}

// ---------------------------------------------------------------- responses

struct Head {
    status: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn parse_status_line(line: &[u8]) -> Result<u16, HttpError> {
    let line = std::str::from_utf8(line).map_err(|_| HttpError::Protocol("bad status line"))?.trim_end();
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    let code = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") || code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HttpError::Protocol("bad status line"));
    }
    code.parse().map_err(|_| HttpError::Protocol("bad status line"))
}

fn read_head(conn: &mut Conn) -> Result<Head, HttpError> {
    // Skip informational responses (e.g. 103 Early Hints).
    for _ in 0..5 {
        let status = parse_status_line(&conn.read_line(MAX_HEADER_LINE)?)?;
        let mut headers = Vec::new();
        let mut total = 0;
        loop {
            let line = conn.read_line(MAX_HEADER_LINE)?;
            if line.is_empty() {
                break;
            }
            total += line.len();
            if headers.len() >= MAX_HEADERS || total > MAX_HEADER_BYTES {
                return Err(HttpError::Protocol("too many headers"));
            }
            let text = String::from_utf8_lossy(&line);
            let (name, value) = text.split_once(':').ok_or(HttpError::Protocol("bad header"))?;
            if name.is_empty() || name.contains(|c: char| c.is_ascii_whitespace()) {
                return Err(HttpError::Protocol("bad header"));
            }
            headers.push((name.to_string(), value.trim().to_string()));
        }
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        return Ok(Head { status, headers });
    }
    Err(HttpError::Protocol("too many informational responses"))
}

enum Framing {
    Length(u64),
    Chunked { left_in_chunk: u64, finished: bool },
    UntilClose { finished: bool },
}

/// A successful response whose body can be streamed with `read`.
pub struct Response {
    conn: Conn,
    framing: Framing,
    /// Declared size, when the server sent Content-Length.
    pub content_length: Option<u64>,
    max_body: u64,
    received: u64,
}

impl Response {
    fn new(conn: Conn, head: &Head, max_body: u64) -> Result<Response, HttpError> {
        if let Some(encoding) = head.header("content-encoding")
            && !encoding.eq_ignore_ascii_case("identity")
        {
            return Err(HttpError::Protocol("compressed response"));
        }
        let framing;
        let mut content_length = None;
        if let Some(te) = head.header("transfer-encoding") {
            if !te.trim().eq_ignore_ascii_case("chunked") {
                return Err(HttpError::Protocol("unsupported transfer encoding"));
            }
            framing = Framing::Chunked { left_in_chunk: 0, finished: false };
        } else {
            let mut lengths = head.all("content-length").map(|v| v.trim());
            match lengths.next() {
                Some(first) => {
                    if lengths.any(|other| other != first) {
                        return Err(HttpError::Protocol("conflicting content lengths"));
                    }
                    if first.is_empty() || first.len() > 19 || !first.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(HttpError::Protocol("bad content length"));
                    }
                    let n: u64 = first.parse().map_err(|_| HttpError::Protocol("bad content length"))?;
                    if n > max_body {
                        return Err(HttpError::TooLarge);
                    }
                    content_length = Some(n);
                    framing = Framing::Length(n);
                }
                None => framing = Framing::UntilClose { finished: false },
            }
        }
        Ok(Response { conn, framing, content_length, max_body, received: 0 })
    }

    /// Read part of the body into `out`. Returns 0 at the end of the body.
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize, HttpError> {
        if out.is_empty() {
            return Ok(0);
        }
        let n = match &mut self.framing {
            Framing::Length(left) => {
                if *left == 0 {
                    return Ok(0);
                }
                let want = (*left).min(out.len() as u64) as usize;
                let n = self.conn.read_some(&mut out[..want])?;
                if n == 0 {
                    return Err(HttpError::Protocol("connection closed early"));
                }
                *left -= n as u64;
                n
            }
            Framing::Chunked { left_in_chunk, finished } => {
                if *finished {
                    return Ok(0);
                }
                if *left_in_chunk == 0 {
                    let size = parse_chunk_size(&self.conn.read_line(1024)?)?;
                    if size == 0 {
                        // Trailers, then the blank line that ends the body.
                        let mut trailer_bytes = 0;
                        loop {
                            let line = self.conn.read_line(MAX_HEADER_LINE)?;
                            trailer_bytes += line.len();
                            if line.is_empty() {
                                break;
                            }
                            if trailer_bytes > MAX_HEADER_BYTES {
                                return Err(HttpError::Protocol("trailers too long"));
                            }
                        }
                        *finished = true;
                        return Ok(0);
                    }
                    *left_in_chunk = size;
                }
                let want = (*left_in_chunk).min(out.len() as u64) as usize;
                let n = self.conn.read_some(&mut out[..want])?;
                if n == 0 {
                    return Err(HttpError::Protocol("connection closed early"));
                }
                *left_in_chunk -= n as u64;
                if *left_in_chunk == 0 && !self.conn.read_line(16)?.is_empty() {
                    return Err(HttpError::Protocol("bad chunk terminator"));
                }
                n
            }
            Framing::UntilClose { finished } => {
                if *finished {
                    return Ok(0);
                }
                // rustls only reports a clean end after the server's
                // close_notify, so a cut connection is an error, not a
                // silently truncated body.
                let n = self.conn.read_some(out)?;
                if n == 0 {
                    *finished = true;
                }
                n
            }
        };
        self.received += n as u64;
        if self.received > self.max_body {
            return Err(HttpError::TooLarge);
        }
        Ok(n)
    }

    /// Read the whole body.
    pub fn read_to_end(mut self) -> Result<Vec<u8>, HttpError> {
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 16 * 1024];
        loop {
            let n = self.read(&mut chunk)?;
            if n == 0 {
                return Ok(body);
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
}

fn parse_chunk_size(line: &[u8]) -> Result<u64, HttpError> {
    let text = std::str::from_utf8(line).map_err(|_| HttpError::Protocol("bad chunk size"))?;
    let digits = text.split(';').next().unwrap_or("").trim();
    if digits.is_empty() || digits.len() > 15 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(HttpError::Protocol("bad chunk size"));
    }
    u64::from_str_radix(digits, 16).map_err(|_| HttpError::Protocol("bad chunk size"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Mock(Cursor<Vec<u8>>, usize);
    impl Read for Mock {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // Hand out small pieces to exercise buffer boundaries.
            let limit = buf.len().min(self.1);
            self.0.read(&mut buf[..limit])
        }
    }
    impl Write for Mock {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn respond(raw: &str, piece: usize, max: u64) -> Result<Vec<u8>, HttpError> {
        let mut conn = Conn::new(
            Box::new(Mock(Cursor::new(raw.as_bytes().to_vec()), piece)),
            Instant::now() + Duration::from_secs(5),
        );
        let head = read_head(&mut conn)?;
        if head.status != 200 {
            return Err(HttpError::Status(head.status));
        }
        Response::new(conn, &head, max)?.read_to_end()
    }

    #[test]
    fn framing() {
        for piece in [1, 3, 7, 4096] {
            let r = respond("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA", piece, 100).unwrap();
            assert_eq!(r, b"hello");
            let r = respond(
                "HTTP/1.1 103 Early Hints\r\nLink: x\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n6\r\n world\r\n0\r\nX-Trailer: y\r\n\r\n",
                piece,
                100,
            )
            .unwrap();
            assert_eq!(r, b"hello world");
        }
        assert!(matches!(respond("HTTP/1.1 200 OK\r\nContent-Length: 500\r\n\r\n", 9, 100), Err(HttpError::TooLarge)));
        assert!(matches!(
            respond("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nFFFFFFFF\r\n", 9, 100).map(|_| ()),
            Err(HttpError::Protocol(_)) | Err(HttpError::TooLarge)
        ));
        assert!(matches!(
            respond("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort", 9, 100),
            Err(HttpError::Protocol(_))
        ));
        assert!(matches!(
            respond("HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab", 9, 100),
            Err(HttpError::Protocol(_))
        ));
        assert!(matches!(respond("HTTP/1.1 404 Not Found\r\n\r\n", 9, 100), Err(HttpError::Status(404))));
        assert!(matches!(respond("SSH-2.0\r\n\r\n", 9, 100), Err(HttpError::Protocol(_))));
        assert!(matches!(
            respond("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n\r\nx", 9, 100),
            Err(HttpError::Protocol(_))
        ));
        let long = format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "a".repeat(10_000));
        assert!(matches!(respond(&long, 4096, 100), Err(HttpError::Protocol(_))));
    }

    #[test]
    fn urls() {
        let u = Url::parse("https://GitHub.com/a/b?c=d#frag").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("github.com", 443, "/a/b?c=d"));
        assert_eq!(Url::parse("https://h:8443").unwrap().path, "/");
        assert!(matches!(Url::parse("http://github.com/"), Err(HttpError::NotHttps)));
        for bad in [
            "file:///c:/x",
            "https://user@h/",
            "https://h:0/",
            "https://h:99999/",
            "https:///x",
            "https://h/a b",
            "ftp://h/",
        ] {
            assert!(Url::parse(bad).is_err(), "{bad}");
        }
        let base = Url::parse("https://github.com/x").unwrap();
        assert_eq!(base.join("/y").unwrap().path, "/y");
        assert_eq!(base.join("https://objects.example.com/z").unwrap().host, "objects.example.com");
        assert!(matches!(base.join("http://evil/"), Err(HttpError::NotHttps)));
        assert!(base.join("javascript:alert(1)").is_err());
        assert!(base.join("relative/path").is_err());
        assert_eq!(base.join("/a?next=https://x").unwrap().host, "github.com");
        let v6 = Url::parse("https://[::1]:8443/x").unwrap();
        assert_eq!(v6.host_header(), "[::1]:8443");
    }

    #[test]
    fn proxies() {
        let p = Proxy::parse("http://us%65r:p%40ss@proxy.corp:3128/").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("proxy.corp", 3128));
        assert_eq!(p.authorization.as_deref(), Some("Basic dXNlcjpwQHNz"));
        assert_eq!(Proxy::parse("proxy:8080").unwrap().port, 8080);
        assert!(Proxy::parse("https://proxy:8080").is_err());
        assert!(no_proxy_matches(".example.com, github.com", "api.github.com"));
        assert!(!no_proxy_matches("hub.com", "github.com"));
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert!(Proxy::is_direct(" Direct "));
    }

    #[test]
    fn windows_proxy_setting() {
        let s = SystemProxySettings { server: "proxy.corp:8080".into(), overrides: "*.local;10.*;<local>".into() };
        assert_eq!(s.server_for_https().as_deref(), Some("proxy.corp:8080"));
        assert!(s.bypass("intranet") && s.bypass("box.local") && s.bypass("10.1.2.3"));
        assert!(!s.bypass("github.com"));
        let per = SystemProxySettings {
            server: "http=h1:80;https=https://h2:443;ftp=h3:21".into(),
            overrides: String::new(),
        };
        assert_eq!(per.server_for_https().as_deref(), Some("h2:443"));
        let http_only = SystemProxySettings { server: "http=h1:80".into(), overrides: String::new() };
        assert_eq!(http_only.server_for_https().as_deref(), Some("h1:80"));
        assert!(glob_match("*.github.com", "api.github.com") && !glob_match("*.github.com", "github.com"));
    }

    /// A server that accepts and then sends one byte every 300 ms must not
    /// hold a request past its total limit.
    #[test]
    fn deadline_bounds_slow_servers() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(1) {
                let mut s = stream.unwrap();
                // A TLS record header promising 16 KiB, then a trickle.
                let _ = s.write_all(b"\x16\x03\x03\x40\x00");
                for _ in 0..100 {
                    if s.write_all(b"\x00").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        });
        let client = Client::new(Options {
            user_agent: "test".into(),
            proxy: Some("direct".into()),
            extra_root_certificates: None,
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_millis(1500),
        })
        .unwrap();
        let started = Instant::now();
        let result = client.get(&format!("https://127.0.0.1:{port}/"), 100);
        assert!(matches!(result, Err(HttpError::TimedOut)), "{:?}", result.err());
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(1400) && took < Duration::from_secs(3), "took {took:?}");
    }

    #[test]
    fn connect_falls_back_to_an_address_that_answers() {
        // The first address never answers (TEST-NET-1 is unroutable; the
        // attempt hangs or fails), the second is a local listener: the
        // connection must come from the second without waiting out the
        // first's 5-second limit.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let good = listener.local_addr().unwrap();
        let dead: SocketAddr = "192.0.2.1:9".parse().unwrap();
        let started = Instant::now();
        let stream = connect_any(vec![dead, good], Instant::now() + Duration::from_secs(10), Duration::from_secs(5))
            .expect("connected");
        assert_eq!(stream.peer_addr().unwrap(), good);
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    #[test]
    fn connect_reports_failure_when_nothing_answers() {
        // A port nobody listens on: refused at once, not a timeout.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let result = connect_any(vec![free], Instant::now() + Duration::from_secs(5), Duration::from_secs(2));
        assert!(matches!(result, Err(HttpError::Connect(_))), "{:?}", result.err());
    }
}
