use wasmtime_wasi_http::{Error as HttpError, RequestOptions, WasiBody, WasiHttpHooks};

use crate::plugin_http::{resolve_pinned, send_pinned};

/// Routes every `wasi:http` request from a plugin through
/// [`resolve_pinned`] and [`send_pinned`]. `WasiHttpHooks::send_request` is the
/// documented interception point and the default sender is never used, because
/// it would resolve the host a second time.
///
/// One implementation serves both engines: `wasmtime_wasi_http` routes p2
/// (action plugins) and p3 (trigger plugins) requests through the same
/// `WasiHttpHooks` trait, and reports rejections through its own
/// version-agnostic [`HttpError`], which it maps to each protocol's
/// `error-code` itself.
pub(super) struct PluginHttpHooks;

impl WasiHttpHooks for PluginHttpHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: Box<dyn std::future::Future<Output = Result<(), HttpError>> + Send>,
    ) -> Box<
        dyn std::future::Future<
                Output = Result<
                    (
                        hyper::Response<WasiBody>,
                        Box<dyn std::future::Future<Output = Result<(), HttpError>> + Send>,
                    ),
                    HttpError,
                >,
            > + Send,
    > {
        // Unused: nothing is sent on rejection, and the upstream default
        // `send_request` discards it identically.
        _ = fut;
        Box::new(async move {
            let addrs = resolve_pinned(request.uri()).await.map_err(HttpError::from)?;
            use http_body_util::BodyExt;
            let (res, io) = send_pinned(request, options, addrs).await?;
            Ok((
                res.map(BodyExt::boxed_unsync),
                Box::new(io) as Box<dyn std::future::Future<Output = Result<(), HttpError>> + Send>,
            ))
        })
    }
}

#[cfg(test)]
mod plugin_http_hooks_tests {
    use super::{HttpError, PluginHttpHooks, WasiBody, WasiHttpHooks};

    async fn send(uri: &str) -> Result<(), HttpError> {
        use http_body_util::{BodyExt, Empty};
        use hyper::body::Bytes;
        use std::future::Future;

        let body: WasiBody = Empty::<Bytes>::new().map_err(|never| match never {}).boxed_unsync();
        let request = hyper::Request::builder().uri(uri).body(body).expect("request build failed");
        let fut: Box<dyn Future<Output = Result<(), HttpError>> + Send> = Box::new(async { Ok(()) });

        let mut hooks = PluginHttpHooks;
        Box::into_pin(hooks.send_request(request, None, fut)).await.map(|_| ())
    }

    #[tokio::test]
    async fn send_request_rejects_private_and_loopback_destinations() {
        for uri in ["http://192.168.1.1/", "http://[::1]:8080/", "http://localhost:9/", "http://169.254.169.254/latest"] {
            match send(uri).await {
                Err(HttpError::DestinationIpProhibited) => {}
                other => panic!("{uri}: expected DestinationIpProhibited, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn send_request_rejects_a_uri_without_a_host() {
        match send("/no-authority").await {
            Err(HttpError::HttpRequestUriInvalid) => {}
            other => panic!("expected HttpRequestUriInvalid, got {other:?}"),
        }
    }
}
