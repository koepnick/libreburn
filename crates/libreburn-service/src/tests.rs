// SPDX-License-Identifier: AGPL-3.0-or-later
//! Router tests. They run the real router in-process, with no hardware.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;

const PROFILE: &str = "[machine]\nname='k40'\ndriver='ruida'\nlaser='co2'\n\
    [work_area]\nwidth_mm=300\nheight_mm=200\n\
    [limits]\nmax_power_pct=60\nmax_speed_mm_s=400\naccel_mm_s2=3000\n\
    [framing]\ndefault_source='laser_off'\n";

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="50mm" viewBox="0 0 50 50">
    <rect x="10" y="10" width="20" height="20" fill="none" stroke="#ff0000"/></svg>"##;

fn state() -> AppState {
    AppState::new(
        None,
        Some(libreburn_core::MachineProfile::from_toml(PROFILE).unwrap()),
    )
}

async fn call(app: &Router, method: &str, uri: &str, body: impl Into<Body>) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(body.into())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, value)
}

#[tokio::test]
async fn health_returns_ok() {
    let (status, body) = call(&router(state()), "GET", "/api/v1/health", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn import_configure_plan_and_fetch_moves() {
    let app = router(state());
    let (status, report) = call(&app, "POST", "/api/v1/project/import?name=box.svg", SVG).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["new_layers"], json!(["#ff0000"]));
    assert_eq!(report["revision"], 1);

    let (status, layer) = call(
        &app,
        "PATCH",
        "/api/v1/project/layers/%23ff0000",
        Body::from(r#"{"settings":{"power_pct":40,"speed_mm_s":20,"passes":2}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{layer}");
    assert_eq!(layer["settings"]["power_pct"], 40.0);
    assert_eq!(
        layer["settings"]["mode"], "cut",
        "untouched fields are kept"
    );

    let (status, plan) = call(&app, "POST", "/api/v1/job/plan", Body::empty()).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["blocked"], false);
    let secs = plan["estimate"]["seconds"].as_f64().unwrap();
    assert!(
        secs > 8.0 && secs < 9.0,
        "160 mm at 20 mm/s plus accel: {secs}"
    );

    let (status, job) = call(&app, "GET", "/api/v1/job/moves", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        job["moves"].as_array().unwrap().len(),
        plan["moves"].as_u64().unwrap() as usize
    );

    let res = app
        .clone()
        .oneshot(
            Request::get("/api/v1/job/moves.bin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        bytes.len(),
        plan["moves"].as_u64().unwrap() as usize * api::BINARY_MOVE_SIZE
    );

    let (_, frame) = call(
        &app,
        "GET",
        "/api/v1/job/frame?shape=bounding_box",
        Body::empty(),
    )
    .await;
    let corner = frame["outline"][0].as_array().unwrap();
    assert!((corner[0].as_f64().unwrap() - 10.0).abs() < 1e-4);
    assert!((corner[1].as_f64().unwrap() - 10.0).abs() < 1e-4);
}

#[tokio::test]
async fn editing_the_project_invalidates_the_plan() {
    let app = router(state());
    let (_, report) = call(&app, "POST", "/api/v1/project/import?name=box.svg", SVG).await;
    let id = report["object_ids"][0].as_u64().unwrap();
    call(&app, "POST", "/api/v1/job/plan", Body::empty()).await;
    let (status, obj) = call(
        &app,
        "PATCH",
        &format!("/api/v1/project/objects/{id}"),
        Body::from(r#"{"transform":[1,0,0,1,5,0]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{obj}");
    assert!((obj["bounds"]["min_x"].as_f64().unwrap() - 15.0).abs() < 1e-4);
    let (status, err) = call(&app, "GET", "/api/v1/job", Body::empty()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["code"], "LB-API-003");
}

#[tokio::test]
async fn errors_are_verbose_json() {
    let app = router(state());
    let (status, err) = call(
        &app,
        "POST",
        "/api/v1/project/import?name=x.pdf",
        "%PDF-1.4",
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(err["code"], "LB-IMPORT-002");
    assert!(err["hint"].as_str().unwrap().contains("SVG"));

    let (status, err) = call(
        &app,
        "PATCH",
        "/api/v1/project/layers/nope",
        Body::from("{}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["code"], "LB-API-002");

    let (status, _) = call(&app, "POST", "/api/v1/job/plan", Body::empty()).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "empty project cannot be planned"
    );
}

#[tokio::test]
async fn planning_without_a_profile_explains_how_to_load_one() {
    let app = router(AppState::new(None, None));
    call(&app, "POST", "/api/v1/project/import?name=box.svg", SVG).await;
    let (status, err) = call(&app, "POST", "/api/v1/job/plan", Body::empty()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(err["hint"].as_str().unwrap().contains("--profile"));
    let (status, _) = call(&app, "PUT", "/api/v1/machine/profile", PROFILE).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "POST", "/api/v1/job/plan", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn bad_profile_is_rejected() {
    let app = router(AppState::new(None, None));
    let (status, err) = call(&app, "PUT", "/api/v1/machine/profile", "nonsense").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["code"], "LB-PROFILE-001");
}

#[tokio::test]
async fn token_is_required_when_set_except_for_health() {
    let app = router(AppState::new(Some("0123456789abcdef".into()), None));
    let (status, _) = call(&app, "GET", "/api/v1/health", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, err) = call(&app, "GET", "/api/v1/project", Body::empty()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(err["code"], "LB-API-004");
    let res = app
        .clone()
        .oneshot(
            Request::get("/api/v1/project")
                .header("authorization", "Bearer 0123456789abcdef")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (status, _) = call(
        &app,
        "GET",
        "/api/v1/project?token=0123456789abcdef",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", "/api/v1/project?token=wrong", Body::empty()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn websocket_announces_changes() {
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events"))
        .await
        .unwrap();
    let mut next = async || loop {
        use futures_util::StreamExt;
        if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
            return serde_json::from_str::<Value>(&t).unwrap();
        }
    };
    assert_eq!(next().await["reason"], "connected");

    let client = format!("http://{addr}/api/v1/project/import?name=box.svg");
    let status = tokio::task::spawn_blocking(move || {
        // Minimal HTTP/1.1 POST without pulling in a client crate.
        use std::io::{Read, Write};
        let url = client.strip_prefix("http://").unwrap();
        let (host, path) = url.split_once('/').unwrap();
        let mut s = std::net::TcpStream::connect(host).unwrap();
        write!(
            s,
            "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SVG}",
            SVG.len()
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    })
    .await
    .unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    let ev = next().await;
    assert_eq!(ev["type"], "project_changed");
    assert_eq!(ev["reason"], "import");
    assert_eq!(ev["revision"], 1);
}
