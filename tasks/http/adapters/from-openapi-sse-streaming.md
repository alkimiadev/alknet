---
id: http/adapters/from-openapi-sse-streaming
name: Implement from_openapi Subscription forwarding as StreamingHandler (SSE response → BoxStream<ResponseEnvelope>)
status: completed
depends_on: [call/registry/streaming-handler-handlerkind]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Branch `from_openapi`'s forwarding handler construction on `op_type` so that a
`Subscription` op (detected via `text/event-stream` response content type)
registers a `StreamingHandler` (`HandlerKind::Stream`) that streams the SSE
response chunks as `ResponseEnvelope::ok()` items. `Query`/`Mutation` ops keep
the existing `Handler` (`HandlerKind::Once`) that returns a single
`ResponseEnvelope`. This closes the gap where a `from_openapi`-imported
`Subscription` returned only the last SSE event.

This task depends on `call/registry/streaming-handler-handlerkind` (which
introduces `HandlerKind::Stream` and `make_streaming_handler`). The existing
`from_openapi` code already detects `Subscription` (`detect_op_type` checks for
`text/event-stream`) and has an SSE parser (`parse_sse_frames`); this task
rewires the subscription path from "collect all events, return last" to "stream
events as they arrive".

### The branch in build_registration

`build_registration` currently always builds a `Handler` (via `make_handler`) and
wraps in `HandlerKind::Once` (after `streaming-handler-handlerkind`). Branch on
`op_type`:

- `Query` / `Mutation` → existing `make_handler` + `forward()` (single response),
  `HandlerKind::Once`
- `Subscription` → new `make_streaming_handler` + `forward_stream()` (SSE
  streaming), `HandlerKind::Stream`

The `op_type` is already computed by `detect_op_type` and available in
`build_registration`. The `HandlerRegistration::new()` call at the end wraps in
the right `HandlerKind` based on `op_type`.

### forward_stream() — the streaming forward function

```rust
async fn forward_stream(
    http_client: &Arc<SharedHttpClient>,
    base_url: &str,
    path_template: &str,
    method: &str,
    auth_scheme: &Option<HttpAuthScheme>,
    default_headers: &HashMap<String, String>,
    namespace: &str,
    error_status_codes: &[(u16, String)],
    input: Value,
    context: OperationContext,
) -> ResponseStream {
    let request_id = context.request_id.clone();

    // 1. Build the request (same as forward())
    let (http_method, url, body, headers) = match build_request(...) {
        Ok(parts) => parts,
        Err(err) => {
            return Box::pin(stream::once(async move {
                ResponseEnvelope::error(request_id, err)
            }));
        }
    };

    // 2. Send with Accept: text/event-stream
    let request_builder = http_client.client()
        .request(http_method, url.as_str())
        .headers(headers)
        .header(ACCEPT, "text/event-stream");
    let request_builder = match body.as_ref() {
        Some(b) => request_builder.body(serde_json::to_string(b).unwrap_or("null".to_string())),
        None => request_builder,
    };

    let response: reqwest::Response = match request_builder.send().await {
        Ok(r) => r,
        Err(err) => {
            return Box::pin(stream::once(async move {
                ResponseEnvelope::error(request_id, CallError::internal(format!("HTTP request failed: {err}")))
            }));
        }
    };

    let status = response.status();
    if !status.is_success() {
        // Non-2xx → single error envelope, stream ends
        let code = error_status_codes.iter()
            .find(|(s, _)| *s == status.as_u16())
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| format!("HTTP_{}", status.as_u16()));
        let message = format!("HTTP {}: {}", status.as_u16(), status.canonical_reason().unwrap_or(""));
        return Box::pin(stream::once(async move {
            ResponseEnvelope::error(request_id, CallError::new(code, message, false))
        }));
    }

    // 3. Stream the SSE chunks → ResponseEnvelope::ok() per data: frame
    let request_id_stream = request_id.clone();
    let sse_stream = response.bytes_stream()
        .scan(String::new(), move |buffer, chunk_result| {
            // Parse SSE frames from the chunk, emit each as a ResponseEnvelope::ok()
            // This is the streaming analogue of stream_subscription()
            let request_id = request_id_stream.clone();
            async move {
                match chunk_result {
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        let (events, remaining) = parse_sse_frames(buffer);
                        *buffer = remaining;
                        // Emit each event as a ResponseEnvelope::ok()
                        let envelopes: Vec<ResponseEnvelope> = events.into_iter()
                            .map(|e| {
                                let parsed = if e.data.trim().is_empty() {
                                    Value::Null
                                } else {
                                    serde_json::from_str(&e.data).unwrap_or(Value::String(e.data.clone()))
                                };
                                ResponseEnvelope::ok(&request_id, parsed)
                            })
                            .collect();
                        Some((envelopes,))  // yield the batch
                    }
                    Err(err) => {
                        let error = CallError::internal(format!("SSE stream error: {err}"));
                        Some(vec![ResponseEnvelope::error(request_id, error)])
                    }
                }
            }
        })
        .flat_map(|envelopes| stream::iter(envelopes));

    Box::pin(sse_stream)
}
```

The exact combinator shape (`scan` + `flat_map`, or a custom `Stream` impl, or
`unfold`) is an implementation detail — the contract is: each SSE `data:` frame
becomes a `ResponseEnvelope::ok()`; an HTTP error (non-2xx) becomes a single
`ResponseEnvelope::error()` and ends the stream; SSE stream end ends the
`ResponseStream` (→ `call.completed` on the wire). Reuse the existing
`parse_sse_frames` parser — it already handles multi-event buffers, partial
trailing lines, comments, multi-line data, BOM.

### Remove stream_subscription() (the collect-all placeholder)

The existing `stream_subscription()` collects all SSE events and returns the
last one as a single `ResponseEnvelope`. This is the placeholder that
truncates. Remove it (or repurpose its SSE-parsing logic into the streaming
`forward_stream`). The `parse_sse_frames` function stays (it's reused by
`forward_stream`); only the collect-all `stream_subscription` wrapper goes.

### build_registration wiring

```rust
let handler = if op_type == OperationType::Subscription {
    // Streaming handler — HandlerKind::Stream
    let stream_handler = make_streaming_handler(move |input, context| {
        // clone captured vars
        async move {
            forward_stream(&http_client, &base_url, &path_template, &method_upper,
                           &auth_scheme, &default_headers, &namespace, &error_status_codes,
                           input, context).await
        }
    });
    HandlerKind::Stream(stream_handler)
} else {
    // Request/response handler — HandlerKind::Once (existing)
    let once_handler = make_handler(move |input, context| {
        // clone captured vars
        async move {
            forward(&http_client, &base_url, &path_template, &method_upper,
                    &auth_scheme, &default_headers, &namespace, &error_status_codes,
                    op_type, input, context).await
        }
    });
    HandlerKind::Once(once_handler)
};

HandlerRegistration::new(spec, handler, OperationProvenance::FromOpenAPI, None, None, capabilities)
```

### What this task does NOT do

- **No `from_mcp` changes.** `from_mcp` handlers are always `HandlerKind::Once`
  (MCP tools are request/response — ADR-041; ADR-049 confirms this is unchanged).
- **No gateway changes.** The gateway `/subscribe` SSE path is
  `http/server/subscribe-sse-streaming`.
- **No `OperationRegistry` changes.** `invoke_streaming()` is provided by
  `call/registry/invoke-streaming`.

## Acceptance Criteria

- [ ] `build_registration` branches on `op_type`: `Subscription` →
      `HandlerKind::Stream` (streaming forward), `Query`/`Mutation` →
      `HandlerKind::Once` (existing forward)
- [ ] `forward_stream()` streams SSE chunks as `ResponseEnvelope::ok()` items
- [ ] Each SSE `data:` frame → one `ResponseEnvelope::ok()`
- [ ] HTTP error (non-2xx) → single `ResponseEnvelope::error()`, stream ends
- [ ] SSE stream end → `ResponseStream` ends (→ `call.completed` on wire)
- [ ] `parse_sse_frames` reused (multi-event, partial trailing, comments,
      multi-line data, BOM — all handled)
- [ ] `stream_subscription()` (collect-all placeholder) removed or repurposed
- [ ] `Query`/`Mutation` forwarding unchanged (existing `forward()` path)
- [ ] `Accept: text/event-stream` header sent for Subscription requests
- [ ] Unit test: `Subscription` op registration is `HandlerKind::Stream`
- [ ] Unit test: `Query` op registration is `HandlerKind::Once` (unchanged)
- [ ] Integration test: `Subscription` forwarding streams multiple
      `ResponseEnvelope::ok()` items from an SSE server (one per `data:` frame)
- [ ] Integration test: `Subscription` forwarding on HTTP error → one
      `ResponseEnvelope::error()`, stream ends
- [ ] Integration test: `Query` forwarding unchanged (single response)
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-http` passes

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049 §9 (from_openapi SSE forwarding)
- docs/architecture/crates/http/http-adapters.md — §Forwarding handler (Subscription → HandlerKind::Stream, SSE → BoxStream)
- docs/architecture/crates/http/http-mcp.md — from_mcp handlers always HandlerKind::Once (unchanged)

## Notes

> The existing `stream_subscription()` is the placeholder that truncates — it
> collects all SSE events and returns the last. Replace it with `forward_stream()`
> that yields each SSE event as a stream item. Reuse `parse_sse_frames` (it's
> already correct for multi-event buffers, partial lines, comments, BOM). The
> combinator shape (`scan` + `flat_map`, `unfold`, or custom `Stream`) is an
> implementation detail — the contract is one `ResponseEnvelope::ok()` per
> `data:` frame, error on HTTP failure, end on SSE close. `from_mcp` is
> unchanged — MCP tools are request/response (ADR-041), always
> `HandlerKind::Once`. The `futures` crate's `StreamExt::scan` / `flat_map` /
> `unfold` are the likely tools.

## Summary

> Branched build_registration on op_type: Subscription → make_streaming_handler + forward_stream() (HandlerKind::Stream), Query/Mutation → existing make_handler + forward() (HandlerKind::Once). forward_stream() sends Accept: text/event-stream, streams SSE chunks via stream::unfold over response.bytes_stream(), reusing parse_sse_frames; each data: frame → one ResponseEnvelope::ok(), HTTP error → single ResponseEnvelope::error(), SSE end → ResponseStream ends. Removed stream_subscription() collect-all placeholder. Added 4 tests + updated integration test. 234 tests pass.