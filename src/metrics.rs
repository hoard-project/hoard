//! Prometheus metrics for Hoard.
//!
//! Exposes metrics on an HTTP endpoint at the configured address.
//! Also serves a `/flush` endpoint to trigger drain in standalone mode.

#![deny(unsafe_code)]

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lazy_static::lazy_static;
use prometheus::{
    register_counter, register_gauge, register_histogram, Counter, Encoder, Gauge, Histogram,
};
use std::convert::Infallible;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

// ── Metric definitions ────────────────────────────────────────────

lazy_static! {
    /// Total number of uploads attempted.
    pub static ref UPLOAD_TOTAL: Counter = register_counter!(
        "hoard_upload_total",
        "Total number of uploads attempted"
    ).expect("duplicate metric: hoard_upload_total");

    /// Total bytes uploaded.
    pub static ref UPLOAD_BYTES_TOTAL: Counter = register_counter!(
        "hoard_upload_bytes_total",
        "Total bytes uploaded"
    ).expect("duplicate metric: hoard_upload_bytes_total");

    /// Number of uploads currently in flight.
    pub static ref UPLOAD_IN_FLIGHT: Gauge = register_gauge!(
        "hoard_upload_in_flight",
        "Uploads currently in progress"
    ).expect("duplicate metric: hoard_upload_in_flight");

    /// Duration of uploads (histogram).
    pub static ref UPLOAD_DURATION_SECONDS: Histogram = register_histogram!(
        "hoard_upload_duration_seconds",
        "Upload duration histogram",
        vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0]
    ).expect("duplicate metric: hoard_upload_duration_seconds");

    /// Total GC cycles completed.
    pub static ref GC_CYCLES_TOTAL: Counter = register_counter!(
        "hoard_gc_cycles_total",
        "Total number of GC cycles completed"
    ).expect("duplicate metric: hoard_gc_cycles_total");

    /// Total objects deleted by GC.
    pub static ref GC_DELETED_TOTAL: Counter = register_counter!(
        "hoard_gc_deleted_total",
        "Total objects deleted by GC"
    ).expect("duplicate metric: hoard_gc_deleted_total");

    /// Total GC errors.
    pub static ref GC_ERRORS_TOTAL: Counter = register_counter!(
        "hoard_gc_errors_total",
        "Total GC errors"
    ).expect("duplicate metric: hoard_gc_errors_total");

    /// RingBuffer events received.
    pub static ref RINGBUF_EVENTS_TOTAL: Counter = register_counter!(
        "hoard_ringbuf_events_total",
        "Total BPF RingBuffer events received"
    ).expect("duplicate metric: hoard_ringbuf_events_total");

    /// Upload failures.
    pub static ref UPLOAD_FAILURES_TOTAL: Counter = register_counter!(
        "hoard_upload_failures_total",
        "Total upload failures"
    ).expect("duplicate metric: hoard_upload_failures_total");
}

// ── Metrics server ─────────────────────────────────────────────────

/// Start the Prometheus metrics HTTP server on the given address.
///
/// If `flush_tx` is provided, a GET/POST `/flush` sends a message to trigger an upload drain.
pub async fn serve_metrics(
    addr: &str,
    flush_tx: Option<mpsc::UnboundedSender<()>>,
) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = addr.parse()?;
    let listener = TcpListener::bind(&addr).await?;

    tracing::info!(%addr, "Prometheus metrics server starting");

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let tx = flush_tx.clone();

        tokio::spawn(async move {
            let svc = service_fn(move |req| metrics_handler(req, tx.clone()));
            if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                tracing::error!(%e, "metrics connection error");
            }
        });
    }
}

/// HTTP handler: GET /metrics → Prometheus text format; GET/POST /flush → trigger drain;
/// GET /health → JSON health check.
async fn metrics_handler(
    req: Request<Incoming>,
    flush_tx: Option<mpsc::UnboundedSender<()>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/flush") | (&Method::GET, "/flush") => {
            if let Some(tx) = &flush_tx {
                let _ = tx.send(());
            }
            Ok(Response::builder()
                .status(StatusCode::OK)
                .body(Full::new(Bytes::from("flush triggered\n")))
                .expect("failed to build flush response"))
        }
        (&Method::GET, "/health") => Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(r#"{"status":"ok"}"#)))
            .expect("failed to build health response")),
        _ => {
            let encoder = prometheus::TextEncoder::new();
            let metric_families = prometheus::gather();
            let mut buffer = Vec::new();
            encoder
                .encode(&metric_families, &mut buffer)
                .unwrap_or_default();
            Ok(Response::builder()
                .status(200)
                .header("Content-Type", "text/plain; version=0.0.4")
                .body(Full::new(Bytes::from(buffer)))
                .expect("failed to build metrics response"))
        }
    }
}
