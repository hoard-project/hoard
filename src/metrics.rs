//! Prometheus metrics for Hoard.
//!
//! Exposes metrics on an HTTP endpoint at the configured address.
//! Also serves a `/flush` endpoint to trigger drain in standalone mode.

#![deny(unsafe_code)]

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, StatusCode};
use lazy_static::lazy_static;
use prometheus::{
    register_counter, register_gauge, register_histogram, Counter, Encoder, Gauge, Histogram,
};
use std::convert::Infallible;
use tokio::sync::mpsc;

// ── Metric definitions ────────────────────────────────────────────

lazy_static! {
    /// Total number of uploads attempted.
    pub static ref UPLOAD_TOTAL: Counter = register_counter!(
        "hoard_upload_total",
        "Total number of uploads attempted"
    ).unwrap();

    /// Total bytes uploaded.
    pub static ref UPLOAD_BYTES_TOTAL: Counter = register_counter!(
        "hoard_upload_bytes_total",
        "Total bytes uploaded"
    ).unwrap();

    /// Number of uploads currently in flight.
    pub static ref UPLOAD_IN_FLIGHT: Gauge = register_gauge!(
        "hoard_upload_in_flight",
        "Uploads currently in progress"
    ).unwrap();

    /// Duration of uploads (histogram).
    pub static ref UPLOAD_DURATION_SECONDS: Histogram = register_histogram!(
        "hoard_upload_duration_seconds",
        "Upload duration histogram",
        vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0]
    ).unwrap();

    /// Total GC cycles completed.
    pub static ref GC_CYCLES_TOTAL: Counter = register_counter!(
        "hoard_gc_cycles_total",
        "Total number of GC cycles completed"
    ).unwrap();

    /// Total objects deleted by GC.
    pub static ref GC_DELETED_TOTAL: Counter = register_counter!(
        "hoard_gc_deleted_total",
        "Total objects deleted by GC"
    ).unwrap();

    /// Total GC errors.
    pub static ref GC_ERRORS_TOTAL: Counter = register_counter!(
        "hoard_gc_errors_total",
        "Total GC errors"
    ).unwrap();

    /// RingBuffer events received.
    pub static ref RINGBUF_EVENTS_TOTAL: Counter = register_counter!(
        "hoard_ringbuf_events_total",
        "Total BPF RingBuffer events received"
    ).unwrap();

    /// Upload failures.
    pub static ref UPLOAD_FAILURES_TOTAL: Counter = register_counter!(
        "hoard_upload_failures_total",
        "Total upload failures"
    ).unwrap();
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

    let make_svc = make_service_fn(move |_conn| {
        let tx = flush_tx.clone();
        async move { Ok::<_, Infallible>(service_fn(move |req| metrics_handler(req, tx.clone()))) }
    });

    tracing::info!(%addr, "Prometheus metrics server starting");

    // hyper 0.14: Server::bind returns Builder, Builder::serve returns Server
    let server = hyper::Server::bind(&addr).serve(make_svc);

    if let Err(e) = server.await {
        tracing::error!(%e, "metrics server error");
    }

    Ok(())
}

/// HTTP handler: GET /metrics → Prometheus text format; GET/POST /flush → trigger drain.
async fn metrics_handler(
    req: Request<Body>,
    flush_tx: Option<mpsc::UnboundedSender<()>>,
) -> Result<Response<Body>, Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/flush") | (&Method::GET, "/flush") => {
            if let Some(tx) = &flush_tx {
                let _ = tx.send(());
            }
            Ok(Response::builder()
                .status(StatusCode::OK)
                .body(Body::from("flush triggered\n"))
                .unwrap())
        }
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
                .body(Body::from(buffer))
                .unwrap())
        }
    }
}
