use futures::future::BoxFuture;
use gpui::http_client::{AsyncBody, HttpClient, Inner, RedirectPolicy, Request, Response, Url};
use http::HeaderValue;
use std::any::type_name;
use std::io::Read as _;
use std::sync::Arc;

/// A minimal [`HttpClient`] implementation backed by the synchronous `ureq` library.
///
/// Blocking calls are offloaded to a thread-pool via `smol::unblock` so the
/// async executor is never stalled.
pub struct UreqHttpClient {
    agent: Arc<ureq::Agent>,
    user_agent: HeaderValue,
}

impl UreqHttpClient {
    pub fn new() -> anyhow::Result<Self> {
        let agent = ureq::Agent::new_with_defaults();
        let user_agent = HeaderValue::from_static("Epoca/0.1");
        Ok(Self {
            agent: Arc::new(agent),
            user_agent,
        })
    }
}

impl HttpClient for UreqHttpClient {
    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let agent = self.agent.clone();

        Box::pin(smol::unblock(move || {
            let uri = req.uri().to_string();
            let method = req.method().as_str().to_string();
            let req_headers: Vec<(String, Vec<u8>)> = req
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
                .collect();

            // Determine whether to follow redirects based on request extensions.
            // ureq 3 follows redirects by default, which is what we want for favicons.
            let _follow = matches!(
                req.extensions().get::<RedirectPolicy>(),
                Some(RedirectPolicy::FollowAll) | Some(RedirectPolicy::FollowLimit(_))
            );

            // Extract request body bytes. GET requests typically have an empty body.
            let body_bytes: Vec<u8> = match req.into_body().0 {
                Inner::Empty => vec![],
                Inner::Bytes(mut cursor) => {
                    let mut buf = Vec::new();
                    cursor.read_to_end(&mut buf).unwrap_or(0);
                    buf
                }
                // AsyncReader cannot be driven from a sync context; treat as empty.
                Inner::AsyncReader(_) => vec![],
            };

            // Build the ureq request.
            let mut builder = ureq::http::Request::builder()
                .method(method.as_str())
                .uri(uri.as_str());

            for (name, value) in &req_headers {
                if let Ok(v) = std::str::from_utf8(value) {
                    builder = builder.header(name.as_str(), v);
                }
            }

            let ureq_req = builder
                .body(body_bytes)
                .map_err(|e| anyhow::anyhow!("request build error: {e}"))?;

            let mut response = agent
                .run(ureq_req)
                .map_err(|e| anyhow::anyhow!("ureq error: {e}"))?;

            // Collect status and headers before consuming the body.
            let status = response.status().as_u16();
            let resp_headers: Vec<(String, Vec<u8>)> = response
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
                .collect();

            // Drain the response body.
            let mut body_buf = Vec::new();
            response
                .body_mut()
                .as_reader()
                .read_to_end(&mut body_buf)
                .map_err(|e| anyhow::anyhow!("read body error: {e}"))?;

            // Build the gpui_http_client Response<AsyncBody>.
            let mut resp_builder = http::Response::builder().status(status);
            for (k, v) in &resp_headers {
                if let Ok(val) = std::str::from_utf8(v) {
                    resp_builder = resp_builder.header(k.as_str(), val);
                }
            }
            let gpui_response = resp_builder
                .body(AsyncBody::from_bytes(body_buf.into()))
                .map_err(|e| anyhow::anyhow!("response build error: {e}"))?;

            Ok(gpui_response)
        }))
    }
}
