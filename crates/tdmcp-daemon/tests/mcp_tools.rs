//! Integration: MCP JSON tool call against in-process router + fake bridge.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::FakeBridgeRpc;
use tdmcp_mcp::{build_mcp_router, AppState, BridgeRpc};
use tower::ServiceExt;

fn registry_with_pid() -> PidRegistry {
    let mut registry = PidRegistry::new();
    registry.handshake(
        34,
        ProcessAttrs {
            title: Some("test".into()),
            fingerprint: ProcessFingerprint {
                title: Some("test".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        Some("1".into()),
        chrono::Utc::now(),
    );
    registry
}

#[tokio::test]
async fn tools_list_includes_derived_input_schema() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/mcp/tools/list")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let tools = v["tools"].as_array().expect("tools array");
    let exec = tools
        .iter()
        .find(|t| t["name"] == "execute_python")
        .expect("execute_python");
    assert!(
        exec["inputSchema"]["properties"]["pid"].is_object(),
        "derived inputSchema must advertise pid: {exec}"
    );
    assert!(exec["inputSchema"]["properties"]["script"].is_object());
}

#[tokio::test]
async fn unknown_field_rejected() {
    let bridge: Arc<dyn BridgeRpc> =
        Arc::new(FakeBridgeRpc::responding(json!({"ok": true, "result": 1})));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"result=1","nope":true}})
                .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn execute_python_happy_path() {
    let bridge: Arc<dyn BridgeRpc> =
        Arc::new(FakeBridgeRpc::responding(json!({"ok": true, "result": 1})));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"result=1","exclusive":false}})
                .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["result"], 1);
}

#[tokio::test]
async fn exclusive_fails_while_shared_in_flight() {
    // Gated bridge: the first (shared) call stays in-flight on the bridge, so
    // the second (exclusive) call must hit queue_busy at enqueue time.
    let fake = FakeBridgeRpc::gated(json!({"ok": true, "result": 1}));
    let gate_handle = fake.gate_handle();
    let bridge: Arc<dyn BridgeRpc> = Arc::new(fake);
    let gate = gate_handle.lock().await;

    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state.clone());

    let app1 = app.clone();
    let first = tokio::spawn(async move {
        let req = Request::builder()
            .method("POST")
            .uri("/mcp/tools/call")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"name":"execute_python","arguments":{"pid":34,"script":"result=1","exclusive":false}})
                    .to_string(),
            ))
            .unwrap();
        app1.oneshot(req).await.unwrap()
    });

    // Let the first call enqueue + reach the gated bridge.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let req2 = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"result=2","exclusive":true}})
                .to_string(),
        ))
        .unwrap();
    let resp2 = app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp2.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["items"][0]["code"], "tdmcp.bridge.queue_busy");
    assert!(v.get("data").is_none());
    assert!(v.get("applied").is_none());

    drop(gate);
    let _ = first.await;
}

#[tokio::test]
async fn script_failure_returns_diagnostics() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(
        json!({"ok": false, "error": "NameError: 'x'", "traceback": "  File \"<td>\", line 1"}),
    ));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name":"execute_python",
                "arguments":{
                    "pid":34,
                    "script":"x",
                    "diagnosticLevel":"detailed"
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["items"][0]["code"], "tdmcp.script.execution_failed");
    assert!(v["items"][0]["rawTraceback"].is_string());
    assert!(v.get("data").is_none());
    assert!(v.get("applied").is_none());
}

#[tokio::test]
async fn script_failure_summary_omits_raw_traceback() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": false,
        "error": "name 'x' is not defined",
        "traceback": "  File \"<td>\", line 1",
        "exception": {
            "type": "NameError",
            "message": "name 'x' is not defined",
            "frames": [],
            "syntax": null,
            "raw": "  File \"<td>\", line 1"
        }
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name":"execute_python",
                "arguments":{
                    "pid":34,
                    "script":"x",
                    "diagnosticLevel":"summary"
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["items"][0]["code"], "tdmcp.script.execution_failed");
    assert!(v["items"][0].get("rawTraceback").is_none());
    assert_eq!(v["items"][0]["exception"]["type"], "NameError");
}

#[tokio::test]
async fn script_failure_default_level_includes_raw_traceback() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": false,
        "error": "boom",
        "traceback": "Traceback (most recent call last):\n  File \"<td>\", line 1",
        "exception": {
            "type": "RuntimeError",
            "message": "boom",
            "frames": [],
            "syntax": null,
            "raw": "Traceback (most recent call last):\n  File \"<td>\", line 1"
        }
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"x"}}).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["items"][0]["rawTraceback"].is_string());
}

#[tokio::test]
async fn mutate_nodes_happy_path() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": true,
        "applied": 3,
        "failedAt": null,
        "steps": [
            {"ok": true, "path": "/project1/noise1"},
            {"ok": true, "path": "/project1/noise1"},
            {"ok": true, "path": "/project1/noise1"}
        ]
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "steps": [
                        {"op": "create", "path": "/project1/noise1", "opType": "noiseTOP"},
                        {"op": "set", "path": "/project1/noise1", "values": {"resolutionw": 128}},
                        {"op": "delete", "path": "/project1/noise1"}
                    ]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["applied"], 3);
    assert!(v["data"]["failedAt"].is_null());
    assert_eq!(v["data"]["steps"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn mutate_nodes_first_step_failure() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": false,
        "applied": 0,
        "failedAt": 0,
        "steps": [
            {"ok": false, "code": "tdmcp.op.unknown_type", "path": "/project1/x", "message": "unknown opType"},
            {"ok": false, "skipped": true, "code": "tdmcp.batch.skipped_dependent", "path": "/project1/x"},
            {"ok": false, "skipped": true, "code": "tdmcp.batch.skipped_dependent", "path": "/project1/x"}
        ]
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "steps": [
                        {"op": "create", "path": "/project1/x", "opType": "notReal"},
                        {"op": "set", "path": "/project1/x", "values": {"a": 1}},
                        {"op": "delete", "path": "/project1/x"}
                    ]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["items"][0]["code"], "tdmcp.op.unknown_type");
    assert_eq!(v["applied"], 0);
    assert_eq!(v["failedAt"], 0);
    assert_eq!(v["steps"][1]["code"], "tdmcp.batch.skipped_dependent");
    assert!(v.get("data").is_none());
}

#[tokio::test]
async fn mutate_nodes_mid_batch_failure() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": false,
        "applied": 1,
        "failedAt": 1,
        "steps": [
            {"ok": true, "path": "/project1/noise1"},
            {"ok": false, "code": "tdmcp.par.unknown", "path": "/project1/noise1", "message": "unknown parameter: nope"},
            {"ok": false, "skipped": true, "code": "tdmcp.batch.skipped_dependent", "path": "/project1/noise1"}
        ]
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "steps": [
                        {"op": "create", "path": "/project1/noise1", "opType": "noiseTOP"},
                        {"op": "set", "path": "/project1/noise1", "values": {"nope": 1}},
                        {"op": "delete", "path": "/project1/noise1"}
                    ]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["items"][0]["code"], "tdmcp.par.unknown");
    assert_eq!(v["applied"], 1);
    assert_eq!(v["failedAt"], 1);
    assert_eq!(v["steps"][2]["code"], "tdmcp.batch.skipped_dependent");
    assert!(v.get("data").is_none());
}

#[tokio::test]
async fn mutate_nodes_unknown_field_rejected() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(
        json!({"ok": true, "applied": 0, "failedAt": null, "steps": []}),
    ));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "steps": [{"op": "delete", "path": "/project1/x", "extra": true}]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mutate_nodes_detail_level_detailed_echoes_values() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": true,
        "applied": 1,
        "failedAt": null,
        "steps": [{
            "ok": true,
            "path": "/project1/noise1",
            "values": {"resolutionw": 128},
            "flags": {"viewer": true, "display": true}
        }]
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "detailLevel": "detailed",
                    "steps": [{
                        "op": "set",
                        "path": "/project1/noise1",
                        "values": {"resolutionw": 128},
                        "flags": {"viewer": true, "display": true}
                    }]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["steps"][0]["values"]["resolutionw"], 128);
    assert_eq!(v["data"]["steps"][0]["flags"]["viewer"], true);
    assert_eq!(v["data"]["steps"][0]["flags"]["display"], true);
}

#[tokio::test]
async fn mutate_nodes_flag_unknown_surfaces_diagnostics() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": false,
        "applied": 0,
        "failedAt": 0,
        "steps": [
            {
                "ok": false,
                "code": "tdmcp.flag.unknown",
                "path": "/project1/noise1",
                "message": "unknown flag: selected",
                "field": "selected"
            }
        ]
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "steps": [{
                        "op": "set",
                        "path": "/project1/noise1",
                        "flags": {"selected": true}
                    }]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["items"][0]["code"], "tdmcp.flag.unknown");
    assert_eq!(v["items"][0]["span"]["field"], "selected");
    assert_eq!(v["failedAt"], 0);
    assert_eq!(v["steps"][0]["code"], "tdmcp.flag.unknown");
    assert!(v.get("data").is_none());
}

#[tokio::test]
async fn mutate_nodes_wrong_collection_lint_forwarded() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": false,
        "applied": 0,
        "failedAt": 0,
        "steps": [{
            "ok": false,
            "code": "tdmcp.par.unknown",
            "path": "/project1/noise1",
            "field": "viewer",
            "message": "unknown parameter: viewer (exists as flag — use flags)",
            "lints": [{
                "severity": "lint",
                "code": "tdmcp.par.wrong_collection",
                "message": "'viewer' is an OP flag; use flags, not values",
                "confidence": "high",
                "suggestion": {"replace": "flags.viewer"}
            }]
        }]
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "steps": [{
                        "op": "set",
                        "path": "/project1/noise1",
                        "values": {"viewer": true}
                    }]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["applied"], 0);
    assert_eq!(v["failedAt"], 0);
    let item = &v["items"][0];
    assert_eq!(item["code"], "tdmcp.par.unknown");
    assert_eq!(item["span"]["field"], "viewer");
    assert_eq!(item["lints"][0]["code"], "tdmcp.par.wrong_collection");
    assert_eq!(item["lints"][0]["suggestion"]["replace"], "flags.viewer");
    assert!(v.get("data").is_none());
}

#[tokio::test]
async fn mutate_nodes_malformed_lints_keep_hard_error() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": false,
        "applied": 0,
        "failedAt": 0,
        "steps": [{
            "ok": false,
            "code": "tdmcp.flag.unknown",
            "path": "/project1/noise1",
            "field": "resolutionw",
            "message": "unknown flag: resolutionw",
            "lints": "bogus"
        }]
    })));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "mutate_nodes",
                "arguments": {
                    "pid": 34,
                    "steps": [{
                        "op": "set",
                        "path": "/project1/noise1",
                        "flags": {"resolutionw": 64}
                    }]
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    let item = &v["items"][0];
    assert_eq!(item["code"], "tdmcp.flag.unknown");
    assert_eq!(item["span"]["field"], "resolutionw");
    assert!(item["lints"].is_null() || item["lints"].as_array().is_some_and(|a| a.is_empty()));
    assert!(v.get("data").is_none());
}

#[tokio::test]
async fn transport_not_connected_clears_queue_for_exclusive() {
    use tdmcp_core::TaskMode;
    use tdmcp_mcp::testing::{BridgeRpcFailure, FakeBridgeRpc};
    use tdmcp_mcp::dispatch_tool;
    use tokio::sync::Mutex;

    let registry = Arc::new(Mutex::new(registry_with_pid()));
    let bridge = FakeBridgeRpc::failing(BridgeRpcFailure::NotConnected, 34);
    let catalog = Catalog::fallback();

    let err = dispatch_tool(
        &registry,
        &catalog,
        &bridge,
        "execute_python",
        json!({"pid": 34, "script": "result=1"}),
    )
    .await
    .expect_err("NotConnected must fail the tool call");
    match err {
        tdmcp_mcp::ToolCallError::Failed(fail) => {
            assert!(
                fail.diagnostics.items[0].code.contains("bridge"),
                "expected bridge transport code, got {:?}",
                fail.diagnostics.items[0].code
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    {
        let mut reg = registry.lock().await;
        let entry = reg.get(34).expect("entry");
        assert!(
            entry.queue.is_empty(),
            "transport fail must not leave a zombie queue slot"
        );
        reg.enqueue(34, "ExclusiveProbe", TaskMode::Exclusive)
            .expect("exclusive must succeed after queue clear");
        let _ = reg.cancel_queue_keep_connected(34);
    }
}
