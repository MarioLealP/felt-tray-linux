//! Her mouth for a voice another app plays (Hermes reading her a mail summary). The app posts the clip, we time
//! her mouth (Rhubarb, on her body) and answer once she's ready; it starts the sound and posts `go` in the same
//! moment, so her mouth starts with it.
//!
//! Loopback only. A web page can't use it: the Origin must be absent or the packaged Hermes app's (`null`,
//! `file://`), anything else is refused and gets no CORS answer, and the Host must be ours (no DNS rebinding).
//! `POST /speak?text=<url-encoded>` (body: the WAV) -> `{"id": n}` when ready, `{"id": null}` when she won't
//! move (the app plays it anyway); `POST /go?id=n` -> 204; `POST /stop` -> 204.

use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A WAV over this is refused (a minute of 32 kHz mono 16-bit is ~4 MB).
const MAX_WAV: usize = 16 * 1024 * 1024;
const MAX_TEXT: usize = 2000;
/// Rhubarb's own limit is 20 s; past this the app plays without her.
const READY_LIMIT: Duration = Duration::from_secs(25);
/// A prepared line the app never starts is dropped after this.
const GO_LIMIT: Duration = Duration::from_secs(30);

/// What this needs from her body (the body's `Handle`; a fake in tests).
pub trait Mouth: Send + Sync + 'static {
    fn prepare(&self, text: &str, wav: Vec<u8>) -> (oneshot::Receiver<()>, oneshot::Sender<()>);
    fn stop(&self);
}

impl Mouth for crate::body::Handle {
    fn prepare(&self, text: &str, wav: Vec<u8>) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        self.external(text, wav)
    }

    fn stop(&self) {
        self.stop_external();
    }
}

struct State<M> {
    mouth: M,
    port: u16,
    next: AtomicU64,
    /// The one prepared line waiting for its `go`.
    pending: std::sync::Mutex<Option<(u64, oneshot::Sender<()>)>>,
}

type Reply = Response<Full<Bytes>>;

/// Serves on 127.0.0.1:`port` until felt-tray exits; a taken port only logs (her voice just stays Hermes-only).
pub async fn serve<M: Mouth>(port: u16, mouth: M) {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("speak: can't listen on 127.0.0.1:{port}: {e}");
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    tracing::info!("speak: listening on 127.0.0.1:{port}");
    run(listener, mouth, port).await;
}

async fn run<M: Mouth>(listener: TcpListener, mouth: M, port: u16) {
    let state = Arc::new(State {
        mouth,
        port,
        next: AtomicU64::new(1),
        pending: std::sync::Mutex::new(None),
    });
    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                tracing::warn!("speak: accept: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let st = state.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| {
                let st = st.clone();
                async move { Ok::<_, Infallible>(handle(&st, req).await) }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), svc)
                .await;
        });
    }
}

fn reply(status: StatusCode, body: &str, cors: bool) -> Reply {
    let mut b = Response::builder().status(status);
    if cors {
        b = b
            .header("Access-Control-Allow-Origin", "null")
            .header("Access-Control-Allow-Methods", "POST")
            .header("Access-Control-Allow-Headers", "Content-Type")
            .header("Access-Control-Allow-Private-Network", "true")
            .header("Access-Control-Max-Age", "600");
    }
    if !body.is_empty() {
        b = b.header("Content-Type", "application/json");
    }
    b.body(Full::new(Bytes::from(body.to_owned())))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

/// No Origin (a local program) or the packaged Hermes app's; `Some(true)` = answer with CORS.
fn origin_ok(req: &Request<Incoming>) -> Option<bool> {
    match req
        .headers()
        .get("origin")
        .map(|v| v.to_str().unwrap_or(""))
    {
        None => Some(false),
        Some("null" | "file://") => Some(true),
        Some(_) => None,
    }
}

fn host_ok(req: &Request<Incoming>, port: u16) -> bool {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn query(req: &Request<Incoming>, key: &str) -> Option<String> {
    let q = req.uri().query()?;
    form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

async fn handle<M: Mouth>(st: &Arc<State<M>>, req: Request<Incoming>) -> Reply {
    let Some(cors) = origin_ok(&req) else {
        return reply(StatusCode::FORBIDDEN, "", false);
    };
    if !host_ok(&req, st.port) {
        return reply(StatusCode::FORBIDDEN, "", false);
    }
    if req.method() == Method::OPTIONS {
        return reply(StatusCode::NO_CONTENT, "", cors);
    }
    if req.method() != Method::POST {
        return reply(StatusCode::METHOD_NOT_ALLOWED, "", cors);
    }
    match req.uri().path() {
        "/speak" => speak(st, req, cors).await,
        "/go" => {
            let id = query(&req, "id").and_then(|v| v.parse::<u64>().ok());
            let waiting = st.pending.lock().expect("pending lock").take();
            match (id, waiting) {
                (Some(id), Some((n, go))) if id == n => {
                    let _ = go.send(());
                    reply(StatusCode::NO_CONTENT, "", cors)
                }
                (_, w) => {
                    *st.pending.lock().expect("pending lock") = w; // not that line: keep it
                    reply(StatusCode::NOT_FOUND, "", cors)
                }
            }
        }
        "/stop" => {
            st.pending.lock().expect("pending lock").take();
            st.mouth.stop();
            reply(StatusCode::NO_CONTENT, "", cors)
        }
        _ => reply(StatusCode::NOT_FOUND, "", cors),
    }
}

async fn speak<M: Mouth>(st: &Arc<State<M>>, req: Request<Incoming>, cors: bool) -> Reply {
    let text = query(&req, "text").unwrap_or_default();
    if text.trim().is_empty() || text.chars().count() > MAX_TEXT {
        return reply(StatusCode::BAD_REQUEST, "", cors);
    }
    let wav = match Limited::new(req.into_body(), MAX_WAV).collect().await {
        Ok(b) => b.to_bytes().to_vec(),
        Err(_) => return reply(StatusCode::PAYLOAD_TOO_LARGE, "", cors),
    };
    if wav.len() < 44 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return reply(StatusCode::UNSUPPORTED_MEDIA_TYPE, "", cors);
    }
    let (ready, go) = st.mouth.prepare(text.trim(), wav);
    match tokio::time::timeout(READY_LIMIT, ready).await {
        Ok(Ok(())) => {
            let id = st.next.fetch_add(1, Ordering::Relaxed);
            *st.pending.lock().expect("pending lock") = Some((id, go));
            let st2 = st.clone();
            tokio::spawn(async move {
                tokio::time::sleep(GO_LIMIT).await;
                let mut p = st2.pending.lock().expect("pending lock");
                if p.as_ref().is_some_and(|(n, _)| *n == id) {
                    p.take(); // never started: her mouth stays shut
                }
            });
            reply(StatusCode::OK, &format!("{{\"id\":{id}}}"), cors)
        }
        _ => reply(StatusCode::OK, "{\"id\":null}", cors),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Default)]
    struct Fake {
        log: std::sync::Mutex<Vec<String>>,
        gos: std::sync::Mutex<Vec<oneshot::Receiver<()>>>,
    }

    impl Mouth for Arc<Fake> {
        fn prepare(
            &self,
            text: &str,
            wav: Vec<u8>,
        ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
            self.log
                .lock()
                .unwrap()
                .push(format!("prepare {text} {}", wav.len()));
            let (rtx, rrx) = oneshot::channel();
            let (gtx, grx) = oneshot::channel();
            let _ = rtx.send(());
            self.gos.lock().unwrap().push(grx);
            (rrx, gtx)
        }
        fn stop(&self) {
            self.log.lock().unwrap().push("stop".into());
        }
    }

    async fn start() -> (u16, Arc<Fake>) {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        let fake = Arc::new(Fake::default());
        tokio::spawn(run(l, fake.clone(), port));
        (port, fake)
    }

    async fn send(port: u16, head: &str, body: &[u8]) -> String {
        let mut s = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let req = format!(
            "{head}\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        s.write_all(body).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    fn wav() -> Vec<u8> {
        let mut w = b"RIFF\0\0\0\0WAVE".to_vec();
        w.resize(64, 0);
        w
    }

    #[tokio::test]
    async fn speak_then_go_starts_her_mouth() {
        let (port, fake) = start().await;
        let r = send(
            port,
            "POST /speak?text=Hi%20Mario HTTP/1.1\r\nOrigin: null",
            &wav(),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(r.contains("{\"id\":1}"), "{r}");
        assert!(
            r.to_ascii_lowercase()
                .contains("access-control-allow-origin: null")
        );
        assert_eq!(fake.log.lock().unwrap()[0], "prepare Hi Mario 64");
        let r = send(port, "POST /go?id=1 HTTP/1.1\r\nOrigin: null", b"").await;
        assert!(r.starts_with("HTTP/1.1 204"), "{r}");
        let go = fake.gos.lock().unwrap().pop().unwrap();
        assert!(go.await.is_ok());
    }

    #[tokio::test]
    async fn web_pages_and_strange_hosts_are_refused() {
        let (port, fake) = start().await;
        let r = send(
            port,
            "POST /speak?text=x HTTP/1.1\r\nOrigin: https://evil.example",
            &wav(),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 403"), "{r}");
        assert!(!r.to_ascii_lowercase().contains("access-control"));
        let mut s = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        s.write_all(b"POST /stop HTTP/1.1\r\nHost: evil.example\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
        assert!(fake.log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn not_a_wav_or_a_wrong_go_is_refused() {
        let (port, fake) = start().await;
        let r = send(
            port,
            "POST /speak?text=x HTTP/1.1",
            b"not a wav at all, just some bytes here, long enough to pass",
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 415"), "{r}");
        let r = send(port, "POST /speak?text=x HTTP/1.1", &wav()).await;
        assert!(r.contains("{\"id\":1}"), "{r}");
        let r = send(port, "POST /go?id=7 HTTP/1.1", b"").await;
        assert!(r.starts_with("HTTP/1.1 404"), "{r}");
        let r = send(port, "POST /go?id=1 HTTP/1.1", b"").await;
        assert!(r.starts_with("HTTP/1.1 204"), "{r}");
        let r = send(port, "POST /stop HTTP/1.1", b"").await;
        assert!(r.starts_with("HTTP/1.1 204"), "{r}");
        assert_eq!(fake.log.lock().unwrap().last().unwrap(), "stop");
    }

    #[tokio::test]
    async fn preflight_from_hermes_gets_cors() {
        let (port, _) = start().await;
        let r = send(
            port,
            "OPTIONS /speak HTTP/1.1\r\nOrigin: null\r\nAccess-Control-Request-Method: POST",
            b"",
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 204"), "{r}");
        assert!(
            r.to_ascii_lowercase()
                .contains("access-control-allow-private-network: true")
        );
    }
}
