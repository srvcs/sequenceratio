use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-sequenceratio";
pub const CONCERN: &str = "sequences: ratios between consecutive terms";
pub const DEPENDS_ON: &[&str] = &["srvcs-floatdivide"];

/// Defensive cap on the number of consecutive-pair iterations. A request that
/// would require more dependency calls than this is rejected with a `500`
/// rather than allowed to fan out unbounded.
const MAX_PAIRS: usize = 100_000;

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub floatdivide_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    /// The sequence of numbers. The ratios between each consecutive pair are
    /// returned; a list with fewer than two elements yields an empty result.
    #[schema(value_type = Object)]
    pub values: Vec<Value>,
}

#[derive(Serialize, ToSchema)]
pub struct RatioResponse {
    #[schema(value_type = Object)]
    pub values: Vec<Value>,
    /// One `f64` ratio per consecutive pair: `result[i] = values[i+1] / values[i]`.
    pub result: Vec<f64>,
}

fn ok(values: Vec<Value>, result: Vec<f64>) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "values": values, "result": result })),
    )
        .into_response()
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

/// Ask `srvcs-floatdivide` to compute `a / b`, returning the `f64` ratio.
///
/// Maps the dependency's failures to the response this service should return:
/// `503` if it is unreachable, the forwarded `422` if `floatdivide` rejects the
/// operands (a non-number, or a zero divisor), and a generic `500` if it
/// returns an unusable body.
async fn ask_floatdivide(url: &str, a: &Value, b: &Value) -> Result<f64, Response> {
    let body = json!({ "a": a, "b": b });
    match client::call(url, &body).await {
        Err(DepError::Unreachable) => Err(degraded("srvcs-floatdivide")),
        Ok((200, body)) => match body.get("result").and_then(Value::as_f64) {
            Some(ratio) => Ok(ratio),
            None => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "srvcs-floatdivide returned no f64 result" })),
            )
                .into_response()),
        },
        // Bad operands (non-number, or division by zero) — floatdivide already
        // judged them; forward its 422.
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded("srvcs-floatdivide")),
    }
}

/// `POST /` — the ratios between consecutive terms of a sequence.
///
/// This service does no arithmetic of its own. For each consecutive pair it
/// asks `srvcs-floatdivide` for `values[i+1] / values[i]` and collects the
/// `f64` results. A list with fewer than two elements yields `[]` and makes no
/// dependency calls. If `floatdivide` rejects a pair (e.g. a zero divisor) the
/// `422` is forwarded; if it is unreachable this service reports itself
/// degraded rather than guessing.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = RatioResponse),
        (status = 422, description = "a pair has a zero divisor or a non-number (forwarded from srvcs-floatdivide)"),
        (status = 500, description = "srvcs-floatdivide returned an unusable response, or the sequence is too long"),
        (status = 503, description = "the srvcs-floatdivide dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    let n = req.values.len();
    // A list with < 2 elements has no consecutive pairs.
    let pairs = n.saturating_sub(1);
    if pairs > MAX_PAIRS {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "sequence too long" })),
        )
            .into_response();
    }

    let mut result: Vec<f64> = Vec::with_capacity(pairs);
    // Index arithmetic (i, i+1) is local; the division itself is the headline
    // operation and goes through srvcs-floatdivide.
    for i in 0..pairs {
        let a = &req.values[i + 1];
        let b = &req.values[i];
        match ask_floatdivide(&deps.floatdivide_url, a, b).await {
            Ok(ratio) => result.push(ratio),
            Err(resp) => return resp,
        }
    }
    ok(req.values, result)
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, RatioResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[tokio::test]
    async fn index_reports_dependency() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-sequenceratio");
        assert_eq!(info.concern, "sequences: ratios between consecutive terms");
        assert_eq!(info.depends_on, vec!["srvcs-floatdivide"]);
    }
}
