use axum::body::Body;
use axum::extract::Json as JsonExtract;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_sequenceratio::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

const DEAD_URL: &str = "http://127.0.0.1:1";

/// Mock `srvcs-floatdivide` that ACTUALLY COMPUTES: it reads `{a, b}` from the
/// request and returns `{"a", "b", "result": a / b}` as an `f64`. A zero
/// divisor is rejected with a `422`, exactly as the real floatdivide does. This
/// is what makes the per-pair loop genuinely testable — the ratios are real,
/// not faked.
async fn spawn_computing_floatdivide() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|JsonExtract(req): JsonExtract<Value>| async move {
            let a = req["a"].as_f64();
            let b = req["b"].as_f64();
            match (a, b) {
                (Some(a), Some(b)) if b != 0.0 => (
                    StatusCode::OK,
                    Json(json!({ "a": a, "b": b, "result": a / b })),
                ),
                (Some(_), Some(_)) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({ "error": "division by zero" })),
                ),
                _ => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({ "error": "value is not a number" })),
                ),
            }
        }),
    );
    serve(app).await
}

async fn serve(app: AxumRouter) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(floatdivide_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            floatdivide_url: floatdivide_url.to_string(),
        },
    )
}

async fn eval(floatdivide_url: &str, values: Value) -> (StatusCode, Value) {
    let res = app(floatdivide_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "values": values }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn index_ok() {
    assert_eq!(status_of("/").await, StatusCode::OK);
}

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn metrics_ok() {
    assert_eq!(status_of("/metrics").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn generates_request_id_when_absent() {
    let res = app(DEAD_URL)
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        res.headers().contains_key("x-request-id"),
        "response must carry a generated x-request-id"
    );
}

// --- Correctness cases from the spec, against a REAL computing floatdivide ---

#[tokio::test]
async fn ratios_of_a_geometric_sequence() {
    let fd = spawn_computing_floatdivide().await;
    let (status, body) = eval(&fd, json!([2, 4, 8])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], json!([2.0, 2.0]));
    assert_eq!(body["values"], json!([2, 4, 8]));
}

#[tokio::test]
async fn fractional_ratios() {
    let fd = spawn_computing_floatdivide().await;
    // 5/2 = 2.5, 1/5 = 0.2
    let (status, body) = eval(&fd, json!([2, 5, 1])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], json!([2.5, 0.2]));
}

#[tokio::test]
async fn empty_list_yields_empty_with_no_calls() {
    // DEAD_URL: if the loop tried to call floatdivide at all on an empty list,
    // this would degrade to 503. It must short-circuit to [] with no calls.
    let (status, body) = eval(DEAD_URL, json!([])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], json!([]));
    assert_eq!(body["values"], json!([]));
}

#[tokio::test]
async fn singleton_yields_empty_with_no_calls() {
    let (status, body) = eval(DEAD_URL, json!([42])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], json!([]));
}

// --- Error / edge cases ---

#[tokio::test]
async fn forwards_422_for_zero_divisor() {
    // The pair (4, 0) divides by zero: values[1] / values[0] = 4 / 0.
    let fd = spawn_computing_floatdivide().await;
    let (status, body) = eval(&fd, json!([0, 4, 8])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "division by zero");
}

#[tokio::test]
async fn forwards_422_for_non_number() {
    let fd = spawn_computing_floatdivide().await;
    let (status, body) = eval(&fd, json!([2, "nope", 8])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "value is not a number");
}

#[tokio::test]
async fn degrades_when_floatdivide_is_unreachable() {
    let (status, body) = eval(DEAD_URL, json!([2, 4, 8])).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-floatdivide");
}
