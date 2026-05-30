# srvcs-sequenceratio

The sequence-ratio service of the srvcs.cloud distributed standard library.

Its single concern: **the ratios between consecutive terms of a sequence.** It
does no arithmetic of its own. For each consecutive pair it asks
[`srvcs-floatdivide`](https://github.com/srvcs/floatdivide) for the quotient:

```text
result = []
for i in 0 .. len(values) - 1:
    result[i] = floatdivide(values[i + 1], values[i])   # one HTTP call per pair
```

A sequence with **fewer than two elements** yields `[]`, and makes no dependency
calls at all. Each ratio in `result` is an `f64`.

```text
sequenceratio([2, 4, 8]) == [2.0, 2.0]
```

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/` | Service identity, concern, and dependency list |
| `POST` | `/` | Ratios between consecutive terms of `values` |
| `GET` | `/healthz` `/readyz` `/metrics` `/openapi.json` | srvcs service standard surface |

```sh
curl -s -X POST localhost:8080/ -H 'content-type: application/json' -d '{"values": [2, 4, 8]}'
# {"values":[2,4,8],"result":[2.0,2.0]}
```

Responses:

- `200 {"values": [...], "result": [...]}` — evaluated; `result` is an array of `f64`.
- `422` — a pair has a zero divisor or a non-number, forwarded from `srvcs-floatdivide`.
- `500` — `srvcs-floatdivide` returned an unusable response, or the sequence is too long.
- `503` — the `srvcs-floatdivide` dependency is unavailable.

## Dependencies

- [`srvcs-floatdivide`](https://github.com/srvcs/floatdivide)

A single request fans out across the dependency graph: one
`sequenceratio → floatdivide` call per consecutive pair, and each `floatdivide`
in turn validates its operands.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SRVCS_BIND_ADDR` | `0.0.0.0:8080` | Bind address |
| `SRVCS_FLOATDIVIDE_URL` | `http://127.0.0.1:8081` | Base URL of `srvcs-floatdivide` |
| `SRVCS_ENV` | `development` | Environment label for logs |
| `RUST_LOG` | `info,tower_http=info` | Tracing filter |

## Local checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Orchestration tests stand up a mock `srvcs-floatdivide` in-process that
**actually computes** `a / b` from the request body, so the per-pair loop is
genuinely exercised (e.g. `sequenceratio([2, 4, 8]) == [2.0, 2.0]`). See
[`srvcs/platform`](https://github.com/srvcs/platform) for the shared standard.

> Note: the `cargoHash` in `flake.nix` is inherited from the template and must be
> refreshed with a `nix build` before the Nix gates pass.
