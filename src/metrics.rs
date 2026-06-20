//! Observability metrics for Hoard.
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
use prometheus::{
    register_counter, register_gauge, register_histogram, Counter, Encoder, Gauge, Histogram,
};
use std::convert::Infallible;
use std::sync::LazyLock;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

// ── Metric definitions ────────────────────────────────────────────

/// Total number of uploads attempted.
pub static UPLOAD_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!("hoard_upload_total", "Total number of uploads attempted")
        .expect("duplicate metric: hoard_upload_total")
});

/// Total bytes uploaded.
pub static UPLOAD_BYTES_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!("hoard_upload_bytes_total", "Total bytes uploaded")
        .expect("duplicate metric: hoard_upload_bytes_total")
});

/// Number of uploads currently in flight.
pub static UPLOAD_IN_FLIGHT: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!("hoard_upload_in_flight", "Uploads currently in progress")
        .expect("duplicate metric: hoard_upload_in_flight")
});

/// Duration of uploads (histogram).
pub static UPLOAD_DURATION_SECONDS: LazyLock<Histogram> = LazyLock::new(|| {
    register_histogram!(
        "hoard_upload_duration_seconds",
        "Upload duration histogram",
        vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0]
    )
    .expect("duplicate metric: hoard_upload_duration_seconds")
});

/// Total GC cycles completed.
pub static GC_CYCLES_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!(
        "hoard_gc_cycles_total",
        "Total number of GC cycles completed"
    )
    .expect("duplicate metric: hoard_gc_cycles_total")
});

/// Total objects deleted by GC.
pub static GC_DELETED_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!("hoard_gc_deleted_total", "Total objects deleted by GC")
        .expect("duplicate metric: hoard_gc_deleted_total")
});

/// Total GC errors.
pub static GC_ERRORS_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!("hoard_gc_errors_total", "Total GC errors")
        .expect("duplicate metric: hoard_gc_errors_total")
});

/// RingBuffer events received.
pub static RINGBUF_EVENTS_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!(
        "hoard_ringbuf_events_total",
        "Total BPF RingBuffer events received"
    )
    .expect("duplicate metric: hoard_ringbuf_events_total")
});

/// Upload failures.
pub static UPLOAD_FAILURES_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!("hoard_upload_failures_total", "Total upload failures")
        .expect("duplicate metric: hoard_upload_failures_total")
});

/// ETag mismatches (silent data corruption detected).
pub static ETAG_MISMATCH_TOTAL: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!(
        "hoard_etag_mismatch_total",
        "Total ETag mismatches (local MD5 ≠ S3 ETag)"
    )
    .expect("duplicate metric: hoard_etag_mismatch_total")
});

/// Current number of files pending upload.
pub static PENDING_FILES: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "hoard_pending_files",
        "Current number of files waiting to be uploaded"
    )
    .expect("duplicate metric: hoard_pending_files")
});

/// Current number of files in dead-letter queue.
pub static DEAD_LETTER_FILES: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "hoard_dead_letter_files",
        "Current number of files in dead-letter queue"
    )
    .expect("duplicate metric: hoard_dead_letter_files")
});

/// Health status: 1 = healthy, 0 = degraded.
pub static HEALTH_STATUS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "hoard_health_status",
        "Health status (1 = healthy, 0 = degraded)"
    )
    .expect("duplicate metric: hoard_health_status")
});

/// Update all derived gauges and health status. Call after any state change.
pub fn update_health_gauges(pending_count: u64, dead_letter_count: u64) {
    PENDING_FILES.set(pending_count as f64);
    DEAD_LETTER_FILES.set(dead_letter_count as f64);

    let mismatches = ETAG_MISMATCH_TOTAL.get();
    let degraded = pending_count > 50 || dead_letter_count > 0 || (mismatches > 0.0);
    HEALTH_STATUS.set(if degraded { 0.0 } else { 1.0 });
}

// ── Metrics server ─────────────────────────────────────────────────

/// Start the metrics HTTP server on the given address.
///
/// If `flush_tx` is provided, a GET/POST `/flush` sends a message to trigger an upload drain.
pub async fn serve_metrics(
    addr: &str,
    flush_tx: Option<mpsc::UnboundedSender<()>>,
) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = addr.parse()?;
    let listener = TcpListener::bind(&addr).await?;

    tracing::info!(%addr, "Metrics server starting");

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

/// HTTP handler: GET /metrics → OpenMetrics text format; GET/POST /flush → trigger drain;
/// GET /health → JSON health check.
async fn metrics_handler(
    req: Request<Incoming>,
    flush_tx: Option<mpsc::UnboundedSender<()>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/flush") | (&Method::GET, "/flush") => {
            if let Some(tx) = flush_tx {
                if tx.send(()).is_err() {
                    tracing::warn!("flush channel closed");
                }
            }
            Ok(Response::builder()
                .status(StatusCode::OK)
                .body(Full::new(Bytes::from(
                    r#"{"status":"ok","message":"flush triggered"}"#,
                )))
                .expect("valid response"))
        }
        (&Method::GET, "/health") => {
            let degraded = HEALTH_STATUS.get() < 1.0;
            let body = serde_json::json!({
                "status": if degraded { "degraded" } else { "ok" },
                "pending": PENDING_FILES.get(),
                "dead_letter": DEAD_LETTER_FILES.get(),
            });
            Ok(Response::builder()
                .status(if degraded {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                })
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(body.to_string())))
                .expect("valid response"))
        }
        _ => {
            let mut buffer = vec![];
            let encoder = prometheus::TextEncoder::new();
            let metric_families = prometheus::gather();
            let _ = encoder.encode(&metric_families, &mut buffer);
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/plain; version=0.0.4")
                .body(Full::new(Bytes::from(buffer)))
                .expect("valid response"))
        }
    }
}
