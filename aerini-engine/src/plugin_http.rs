//! Outbound HTTP for WASM plugins: resolve once, filter, connect to the
//! validated address.
//!
//! `wasmtime_wasi_http::default_send_request` resolves the host a second time
//! at connect time, so a name that answers with a public address for the SSRF
//! check and a private one for the connect would pass. [`send_pinned`] connects
//! to the exact addresses [`resolve_pinned`] validated and keeps the original
//! host name for the TLS server name, so certificate verification and SNI are
//! unchanged.
//!
//! The sender mirrors `wasmtime_wasi_http` 49.0.2 (`default_send_request`),
//! which `aerini-engine/Cargo.toml` pins exactly. Re-check this file against
//! upstream whenever that pin moves.

use std::future::{Future, poll_fn};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};
use hyper::http::uri::Scheme;
use hyper::{Request, Response, Uri};
use once_cell::sync::Lazy;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use wasmtime_wasi_http::io::TokioIo;
use wasmtime_wasi_http::{Error as HttpError, RequestOptions, WasiBody};

use crate::nodes::util::{SsrfPolicy, check_ssrf_ip};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
const DNS_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a plugin request was refused before any connection was made.
#[derive(Debug)]
pub(crate) enum SsrfRejection {
    UriInvalid,
    Prohibited,
    NotFound,
    DnsTimeout,
}

impl From<SsrfRejection> for HttpError {
    fn from(rejection: SsrfRejection) -> Self {
        match rejection {
            SsrfRejection::UriInvalid => HttpError::HttpRequestUriInvalid,
            SsrfRejection::Prohibited => HttpError::DestinationIpProhibited,
            SsrfRejection::NotFound => HttpError::DestinationNotFound,
            SsrfRejection::DnsTimeout => HttpError::DnsTimeout,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Target {
    Literal(SocketAddr),
    Name { host: String, port: u16 },
}

/// Splits a request URI into an already-validated IP literal or a host name
/// that still has to be resolved. Names that are blocked outright
/// (`localhost`, the GCP metadata host) are refused here, before any lookup.
fn classify(uri: &Uri) -> Result<Target, SsrfRejection> {
    let host = uri.host().ok_or(SsrfRejection::UriInvalid)?;
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("https") { 443 } else { 80 });

    // `Uri::host()` keeps the brackets around an IPv6 literal.
    let bare = host.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        check_ssrf_ip(ip, SsrfPolicy::Strict).map_err(|_| SsrfRejection::Prohibited)?;
        return Ok(Target::Literal(SocketAddr::new(ip, port)));
    }

    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower == "metadata.google.internal" {
        return Err(SsrfRejection::Prohibited);
    }
    Ok(Target::Name { host: host.to_owned(), port })
}

/// Keeps the resolved addresses only if every one of them passes the Strict
/// policy. One blocked address rejects the whole answer, so a name that
/// mixes public and private records cannot reach the private one.
fn pinned_addrs(resolved: impl IntoIterator<Item = SocketAddr>) -> Result<Vec<SocketAddr>, SsrfRejection> {
    let mut kept = Vec::new();
    for addr in resolved {
        check_ssrf_ip(addr.ip(), SsrfPolicy::Strict).map_err(|_| SsrfRejection::Prohibited)?;
        kept.push(addr);
    }
    if kept.is_empty() {
        return Err(SsrfRejection::NotFound);
    }
    Ok(kept)
}

/// Resolves the request's host once and returns the addresses it may connect
/// to. Fails closed: an unparsable URI, a failed or empty resolution, a
/// resolution slower than ten seconds and any blocked address all reject.
pub(crate) async fn resolve_pinned(uri: &Uri) -> Result<Vec<SocketAddr>, SsrfRejection> {
    match classify(uri)? {
        Target::Literal(addr) => Ok(vec![addr]),
        Target::Name { host, port } => {
            let lookup = tokio::net::lookup_host((host.as_str(), port));
            let resolved = tokio::time::timeout(DNS_TIMEOUT, lookup)
                .await
                .map_err(|_| SsrfRejection::DnsTimeout)?
                .map_err(|_| SsrfRejection::NotFound)?;
            pinned_addrs(resolved)
        }
    }
}

trait Stream: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static {}
impl<T> Stream for T where T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static {}

static TLS_CONFIG: Lazy<Arc<rustls::ClientConfig>> = Lazy::new(|| {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.into() };
    // The provider is named explicitly: this build links both ring and aws-lc-rs,
    // and `ClientConfig::builder()` panics when no process default is installed.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default TLS protocol versions");
    Arc::new(builder.with_root_certificates(roots).with_no_client_auth())
});

fn tls_server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>, HttpError> {
    use rustls::pki_types::ServerName;

    let bare = host.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(ServerName::from(ip));
    }
    Ok(ServerName::try_from(bare).map_err(|_| HttpError::HttpRequestUriInvalid)?.to_owned())
}

async fn connect_any(addrs: &[SocketAddr]) -> Result<TcpStream, HttpError> {
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(HttpError::Connect(
        last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address to connect to")),
    ))
}

async fn open_stream(addrs: &[SocketAddr], tls_host: Option<&str>) -> Result<Box<dyn Stream>, HttpError> {
    let tcp = connect_any(addrs).await?;
    let Some(host) = tls_host else {
        return Ok(Box::new(tcp));
    };
    let name = tls_server_name(host)?;
    let connector = tokio_rustls::TlsConnector::from(TLS_CONFIG.clone());
    let tls = connector.connect(name, tcp).await.map_err(HttpError::Tls)?;
    Ok(Box::new(tls))
}

/// Response body that fails with a read timeout when no frame arrives within
/// the between-bytes interval.
pub(crate) struct IncomingResponseBody {
    incoming: Incoming,
    timeout: tokio::time::Interval,
}

impl Body for IncomingResponseBody {
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        match Pin::new(&mut self.as_mut().incoming).poll_frame(cx) {
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Err(err))) => {
                let err = if err.is_timeout() { HttpError::HttpResponseTimeout } else { HttpError::from(err) };
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(Some(Ok(frame))) => {
                self.timeout.reset();
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => {
                ready!(self.timeout.poll_tick(cx));
                Poll::Ready(Some(Err(HttpError::ConnectionReadTimeout)))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.incoming.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.incoming.size_hint()
    }
}

/// Sends `req` over a connection to one of `addrs`, which must come from
/// [`resolve_pinned`]. The request URI's host is used only for the TLS server
/// name; it is never resolved again.
pub(crate) async fn send_pinned(
    mut req: Request<WasiBody>,
    options: Option<RequestOptions>,
    addrs: Vec<SocketAddr>,
) -> Result<(Response<IncomingResponseBody>, impl Future<Output = Result<(), HttpError>> + Send), HttpError> {
    let use_tls = req.uri().scheme() == Some(&Scheme::HTTPS);
    let host = req.uri().host().ok_or(HttpError::HttpRequestUriInvalid)?.to_owned();

    let connect_timeout = options.and_then(|o| o.connect_timeout).unwrap_or(DEFAULT_TIMEOUT);
    let first_byte_timeout = options.and_then(|o| o.first_byte_timeout).unwrap_or(DEFAULT_TIMEOUT);
    let between_bytes_timeout = options.and_then(|o| o.between_bytes_timeout).unwrap_or(DEFAULT_TIMEOUT);

    let stream = tokio::time::timeout(connect_timeout, open_stream(&addrs, use_tls.then_some(host.as_str())))
        .await
        .map_err(|_| HttpError::ConnectionTimeout)??;

    let (mut sender, conn) = tokio::time::timeout(
        connect_timeout,
        hyper::client::conn::http1::Builder::new().handshake(TokioIo::new(stream)),
    )
    .await
    .map_err(|_| HttpError::ConnectionTimeout)??;

    // The request line carries only the path; scheme and authority are for
    // proxies.
    *req.uri_mut() = Uri::builder()
        .path_and_query(req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/"))
        .build()
        .map_err(|_| HttpError::HttpRequestUriInvalid)?;

    let send = async move {
        let res = tokio::time::timeout(first_byte_timeout, sender.send_request(req))
            .await
            .map_err(|_| HttpError::ConnectionReadTimeout)?
            .map_err(HttpError::from)?;
        // `tokio::time::interval` panics on a zero period.
        let between_bytes_timeout =
            if between_bytes_timeout == Duration::ZERO { Duration::new(0, 1) } else { between_bytes_timeout };
        let mut timeout = tokio::time::interval(between_bytes_timeout);
        timeout.reset();
        Ok::<_, HttpError>(res.map(|incoming| IncomingResponseBody { incoming, timeout }))
    };
    let mut send = pin!(send);
    let mut conn = Some(conn);
    // Drive the connection while waiting for the response.
    let res = poll_fn(|cx| match send.as_mut().poll(cx) {
        Poll::Ready(Ok(res)) => Poll::Ready(Ok(res)),
        Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
        Poll::Pending => {
            let Some(fut) = conn.as_mut() else {
                return Poll::Pending;
            };
            let res = ready!(Pin::new(fut).poll(cx));
            conn = None;
            match res {
                Ok(()) => send.as_mut().poll(cx),
                Err(err) => Poll::Ready(Err(HttpError::from(err))),
            }
        }
    })
    .await?;

    Ok((res, async move {
        let Some(conn) = conn.take() else {
            return Ok(());
        };
        if let Err(err) = conn.await {
            if err.is_timeout() {
                return Err(HttpError::HttpResponseTimeout);
            }
            return Err(err.into());
        }
        Ok(())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn public_literal_is_pinned_to_itself_with_the_uri_port() {
        assert_eq!(classify(&uri("http://93.184.216.34/")).unwrap(), Target::Literal(sa("93.184.216.34:80")));
        assert_eq!(classify(&uri("https://93.184.216.34/")).unwrap(), Target::Literal(sa("93.184.216.34:443")));
        assert_eq!(classify(&uri("http://93.184.216.34:8080/")).unwrap(), Target::Literal(sa("93.184.216.34:8080")));
    }

    #[test]
    fn blocked_literals_and_names_are_prohibited_without_a_lookup() {
        for u in [
            "http://192.168.1.1/",
            "http://169.254.169.254/",
            "http://[::1]:8080/",
            "http://localhost/",
            "http://app.localhost/",
            "http://metadata.google.internal/",
        ] {
            assert!(matches!(classify(&uri(u)), Err(SsrfRejection::Prohibited)), "{u}");
        }
    }

    #[test]
    fn uri_without_host_is_invalid() {
        assert!(matches!(classify(&uri("/no-authority")), Err(SsrfRejection::UriInvalid)));
    }

    #[test]
    fn domain_name_defers_to_resolution() {
        assert_eq!(
            classify(&uri("https://example.com/x")).unwrap(),
            Target::Name { host: "example.com".into(), port: 443 }
        );
    }

    #[test]
    fn pinned_addrs_keeps_an_all_public_answer_in_order() {
        let got = pinned_addrs([sa("93.184.216.34:443"), sa("[2606:2800:220:1:248:1893:25c8:1946]:443")]).unwrap();
        assert_eq!(got, vec![sa("93.184.216.34:443"), sa("[2606:2800:220:1:248:1893:25c8:1946]:443")]);
    }

    #[test]
    fn pinned_addrs_rejects_an_answer_that_mixes_public_and_private() {
        assert!(matches!(
            pinned_addrs([sa("93.184.216.34:443"), sa("10.0.0.5:443")]),
            Err(SsrfRejection::Prohibited)
        ));
        assert!(matches!(
            pinned_addrs([sa("10.0.0.5:443"), sa("93.184.216.34:443")]),
            Err(SsrfRejection::Prohibited)
        ));
    }

    #[test]
    fn pinned_addrs_rejects_an_empty_answer() {
        assert!(matches!(pinned_addrs(Vec::new()), Err(SsrfRejection::NotFound)));
    }

    #[test]
    fn tls_server_name_uses_the_host_not_the_pinned_address() {
        assert!(matches!(tls_server_name("example.com"), Ok(rustls::pki_types::ServerName::DnsName(_))));
        assert!(matches!(tls_server_name("93.184.216.34"), Ok(rustls::pki_types::ServerName::IpAddress(_))));
        assert!(matches!(tls_server_name("[2606:2800:220:1::1]"), Ok(rustls::pki_types::ServerName::IpAddress(_))));
        assert!(tls_server_name("not a host").is_err());
    }

    #[tokio::test]
    async fn resolve_pinned_refuses_localhost_before_any_lookup() {
        assert!(matches!(resolve_pinned(&uri("http://localhost:9/")).await, Err(SsrfRejection::Prohibited)));
    }
}
