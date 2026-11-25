// SPDX-License-Identifier: AGPL-3.0-or-later
//! Project and job endpoints (`/api/v1/project/*`, `/api/v1/job/*`) and the
//! `/api/v1/events` WebSocket.
//!
//! The service holds the only copy of the project (AR-1). Every change bumps
//! a revision number and is announced on the event stream, so every client
//! can stay in step with the others.

use std::sync::{Arc, Mutex, MutexGuard};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get, routing::post};
use libreburn_core::geometry::Bounds;
use libreburn_core::{
    Error, ErrorCode, FrameShape, ImportOptions, MachineProfile, Plan, PlanOptions, Project,
    ProjectLayer, frame_outline,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// Largest accepted upload.
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Shared service state.
#[derive(Debug, Clone)]
pub struct AppState {
    inner: Arc<Mutex<Inner>>,
    events: broadcast::Sender<Event>,
    token: Option<Arc<str>>,
}

#[derive(Debug, Default)]
struct Inner {
    project: Project,
    profile: Option<MachineProfile>,
    plan: Option<Plan>,
    revision: u64,
}

/// A change announced on `/api/v1/events`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The project changed. Clients re-fetch what they show.
    ProjectChanged {
        /// New revision.
        revision: u64,
        /// What happened, such as `import` or `layer_updated`.
        reason: String,
    },
    /// The machine profile changed.
    ProfileChanged {
        /// New revision.
        revision: u64,
    },
    /// A plan finished.
    PlanReady {
        /// Revision the plan was built from.
        revision: u64,
        /// Its summary.
        summary: Box<PlanSummary>,
    },
}

impl AppState {
    /// New state. With a token, every request but the health check must
    /// send `Authorization: Bearer <token>` (AR-4).
    pub fn new(token: Option<String>, profile: Option<MachineProfile>) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                profile,
                ..Default::default()
            })),
            events,
            token: token.map(Into::into),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain data behind; keep serving.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self, inner: &mut Inner, reason: &str) -> u64 {
        inner.revision += 1;
        inner.plan = None;
        let _ = self.events.send(Event::ProjectChanged {
            revision: inner.revision,
            reason: reason.into(),
        });
        inner.revision
    }

    /// The configured access token, if any.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

/// An API error: the core error as JSON with an HTTP status (NF-1).
#[derive(Debug)]
pub struct ApiError(StatusCode, Error);

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    what: &'a str,
    why: &'a str,
    hint: &'a str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let e = &self.1;
        let body = ErrorBody {
            code: e.code.as_str(),
            what: &e.what,
            why: &e.why,
            hint: &e.hint,
            message: e.to_string(),
        };
        (self.0, Json(body)).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let status = match e.code {
            ErrorCode::ImportUnsupported => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ErrorCode::ImportFailed | ErrorCode::PlanFailed | ErrorCode::ProfileInvalid => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ErrorCode::SafetyBlocked => StatusCode::FORBIDDEN,
            ErrorCode::ApiInvalid => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ApiNotFound => StatusCode::NOT_FOUND,
            ErrorCode::ApiConflict => StatusCode::CONFLICT,
            ErrorCode::ApiUnauthorized => StatusCode::UNAUTHORIZED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(status, e)
    }
}

fn api_error(status: StatusCode, what: &str, why: impl Into<String>, hint: &str) -> ApiError {
    let code = match status {
        StatusCode::NOT_FOUND => ErrorCode::ApiNotFound,
        StatusCode::CONFLICT => ErrorCode::ApiConflict,
        StatusCode::UNAUTHORIZED => ErrorCode::ApiUnauthorized,
        StatusCode::INTERNAL_SERVER_ERROR => ErrorCode::ProtocolViolation,
        _ => ErrorCode::ApiInvalid,
    };
    ApiError(status, Error::new(code, what, why, hint))
}

/// An error response for `status`, for use outside this module.
pub fn error_response(status: StatusCode, what: &str, why: &str, hint: &str) -> Response {
    api_error(status, what, why, hint).into_response()
}

fn not_found(what: String, hint: &str) -> ApiError {
    api_error(StatusCode::NOT_FOUND, &what, "it does not exist", hint)
}

type ApiResult<T> = Result<T, ApiError>;

/// Routes under `/api/v1` that need state.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/project", get(get_project).delete(clear_project))
        .route("/api/v1/project/summary", get(project_summary))
        .route("/api/v1/project/import", post(import))
        .route(
            "/api/v1/project/layers/{key}",
            get(get_layer).patch(patch_layer),
        )
        .route(
            "/api/v1/project/objects/{id}",
            get(get_object).patch(patch_object).delete(delete_object),
        )
        .route("/api/v1/machine/profile", get(get_profile).put(put_profile))
        .route("/api/v1/job/plan", post(plan_job))
        .route("/api/v1/job", get(get_plan))
        .route("/api/v1/job/moves", get(get_moves))
        .route("/api/v1/job/moves.bin", get(get_moves_bin))
        .route("/api/v1/job/frame", get(get_frame))
        .route("/api/v1/events", get(events))
}

// ----------------------------------------------------------------- project

#[derive(Serialize)]
struct Revisioned<T> {
    revision: u64,
    #[serde(flatten)]
    body: T,
}

async fn get_project(State(s): State<AppState>) -> Json<Revisioned<Project>> {
    let inner = s.lock();
    Json(Revisioned {
        revision: inner.revision,
        body: inner.project.clone(),
    })
}

async fn clear_project(State(s): State<AppState>) -> Json<Value> {
    let mut inner = s.lock();
    inner.project = Project::new();
    let revision = s.changed(&mut inner, "cleared");
    Json(serde_json::json!({ "revision": revision }))
}

/// An object without its geometry, for lists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectSummary {
    /// Object id.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Layer key.
    pub layer: String,
    /// `vector` or `raster`.
    pub kind: String,
    /// Bounds in mm after the transform.
    pub bounds: Option<Bounds>,
}

/// The project without geometry: what a layer list or object tree needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSummary {
    /// Revision.
    pub revision: u64,
    /// Layers in run order.
    pub layers: Vec<ProjectLayer>,
    /// Objects in drawing order.
    pub objects: Vec<ObjectSummary>,
    /// Bounds of everything on output layers.
    pub bounds: Option<Bounds>,
}

fn summarize(inner: &Inner) -> ProjectSummary {
    ProjectSummary {
        revision: inner.revision,
        layers: inner.project.layers.clone(),
        objects: inner
            .project
            .objects
            .iter()
            .map(|o| ObjectSummary {
                id: o.id,
                name: o.name.clone(),
                layer: o.layer.clone(),
                kind: match o.kind {
                    libreburn_core::ObjectKind::Vector { .. } => "vector".into(),
                    libreburn_core::ObjectKind::Raster { .. } => "raster".into(),
                },
                bounds: o.bounds(),
            })
            .collect(),
        bounds: inner.project.bounds(),
    }
}

async fn project_summary(State(s): State<AppState>) -> Json<ProjectSummary> {
    Json(summarize(&s.lock()))
}

#[derive(Debug, Deserialize)]
struct ImportQuery {
    name: String,
    #[serde(flatten)]
    options: ImportOptions,
}

async fn import(
    State(s): State<AppState>,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    // Parsing can take a while for big files; keep it off the async workers.
    let name = q.name.clone();
    let (parsed, parsed_report) = tokio::task::spawn_blocking(move || {
        let mut scratch = Project::new();
        scratch
            .import(&body, &name, &q.options)
            .map(|r| (scratch, r))
    })
    .await
    .map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Import crashed",
            e.to_string(),
            "report this with the file",
        )
    })??;
    // Merge under the lock so ids and layers stay consistent.
    let mut inner = s.lock();
    let mut report = libreburn_core::project::ImportReport {
        warnings: parsed_report.warnings,
        ..Default::default()
    };
    for layer in &parsed.layers {
        if inner.project.layer(&layer.key).is_none() {
            report.new_layers.push(layer.key.clone());
        }
    }
    let Project {
        layers, objects, ..
    } = parsed;
    for obj in objects {
        let layer = layers
            .iter()
            .find(|l| l.key == obj.layer)
            .cloned()
            .unwrap_or_else(|| {
                ProjectLayer::new(
                    &obj.layer,
                    &obj.layer,
                    "#000000",
                    libreburn_core::LayerMode::Cut,
                )
            });
        report.object_ids.push(
            inner
                .project
                .add_object(obj.name, &layer, obj.transform, obj.kind),
        );
    }
    let revision = s.changed(&mut inner, "import");
    let mut out = serde_json::to_value(&report).unwrap_or_default();
    out["revision"] = revision.into();
    Ok(Json(out))
}

/// Parse a JSON body. Any content type is accepted, so `curl -d` works.
fn parse_json(body: &[u8]) -> ApiResult<Value> {
    serde_json::from_slice(body).map_err(|e| {
        api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Could not read the request body",
            format!("it is not valid JSON: {e}"),
            "send a JSON object with the fields to change",
        )
    })
}

/// Merge `patch` into `target` (RFC 7386 JSON merge patch).
fn merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    t.remove(&k);
                } else {
                    merge(t.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (t, p) => *t = p,
    }
}

fn apply_patch<T: Serialize + for<'de> Deserialize<'de>>(
    current: &T,
    patch: Value,
    what: &str,
) -> ApiResult<T> {
    let mut value = serde_json::to_value(current).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            what,
            e.to_string(),
            "report this as a bug",
        )
    })?;
    merge(&mut value, patch);
    serde_json::from_value(value).map_err(|e| {
        api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("Could not update {what}"),
            e.to_string(),
            "check the field names and types against the API schema",
        )
    })
}

async fn get_layer(
    State(s): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult<Json<ProjectLayer>> {
    let inner = s.lock();
    inner.project.layer(&key).cloned().map(Json).ok_or_else(|| {
        not_found(
            format!("Layer \"{key}\""),
            "list layers with GET /api/v1/project/summary",
        )
    })
}

async fn patch_layer(
    State(s): State<AppState>,
    Path(key): Path<String>,
    body: Bytes,
) -> ApiResult<Json<ProjectLayer>> {
    let patch = parse_json(&body)?;
    let mut inner = s.lock();
    let current = inner.project.layer(&key).ok_or_else(|| {
        not_found(
            format!("Layer \"{key}\""),
            "list layers with GET /api/v1/project/summary",
        )
    })?;
    let updated: ProjectLayer = apply_patch(current, patch, &format!("layer \"{key}\""))?;
    if updated.key != key {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Layer keys cannot change",
            format!("tried to rename \"{key}\" to \"{}\"", updated.key),
            "change settings.name to rename the layer for display",
        ));
    }
    *inner.project.layer_mut(&key).expect("checked above") = updated.clone();
    s.changed(&mut inner, "layer_updated");
    Ok(Json(updated))
}

async fn get_object(
    State(s): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<libreburn_core::Object>> {
    s.lock()
        .project
        .object(id)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
            not_found(
                format!("Object {id}"),
                "list objects with GET /api/v1/project/summary",
            )
        })
}

/// Fields of an object a client may change.
#[derive(Debug, Serialize, Deserialize)]
struct ObjectPatch {
    name: String,
    layer: String,
    transform: kurbo::Affine,
}

async fn patch_object(
    State(s): State<AppState>,
    Path(id): Path<u64>,
    body: Bytes,
) -> ApiResult<Json<ObjectSummary>> {
    let patch = parse_json(&body)?;
    let mut inner = s.lock();
    let obj = inner.project.object(id).ok_or_else(|| {
        not_found(
            format!("Object {id}"),
            "list objects with GET /api/v1/project/summary",
        )
    })?;
    let current = ObjectPatch {
        name: obj.name.clone(),
        layer: obj.layer.clone(),
        transform: obj.transform,
    };
    let updated: ObjectPatch = apply_patch(&current, patch, &format!("object {id}"))?;
    if inner.project.layer(&updated.layer).is_none() {
        return Err(not_found(
            format!("Layer \"{}\"", updated.layer),
            "move objects only to existing layers",
        ));
    }
    let obj = inner.project.object_mut(id).expect("checked above");
    obj.name = updated.name;
    obj.layer = updated.layer;
    obj.transform = updated.transform;
    s.changed(&mut inner, "object_updated");
    let summary = summarize(&inner);
    Ok(Json(
        summary
            .objects
            .into_iter()
            .find(|o| o.id == id)
            .expect("object exists"),
    ))
}

async fn delete_object(State(s): State<AppState>, Path(id): Path<u64>) -> ApiResult<StatusCode> {
    let mut inner = s.lock();
    if !inner.project.remove_object(id) {
        return Err(not_found(
            format!("Object {id}"),
            "list objects with GET /api/v1/project/summary",
        ));
    }
    s.changed(&mut inner, "object_deleted");
    Ok(StatusCode::NO_CONTENT)
}

// ----------------------------------------------------------------- machine

async fn get_profile(State(s): State<AppState>) -> ApiResult<Json<MachineProfile>> {
    s.lock().profile.clone().map(Json).ok_or_else(no_profile)
}

fn no_profile() -> ApiError {
    api_error(
        StatusCode::CONFLICT,
        "No machine profile is loaded",
        "the service was started without --profile",
        "start libreburnd with --profile profiles/<machine>.toml, or PUT the TOML to /api/v1/machine/profile",
    )
}

async fn put_profile(State(s): State<AppState>, body: String) -> ApiResult<Json<MachineProfile>> {
    let profile = MachineProfile::from_toml(&body)?;
    let mut inner = s.lock();
    inner.profile = Some(profile.clone());
    inner.plan = None;
    inner.revision += 1;
    let _ = s.events.send(Event::ProfileChanged {
        revision: inner.revision,
    });
    Ok(Json(profile))
}

// ----------------------------------------------------------------- job

/// A plan without its moves.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanSummary {
    /// Project revision the plan was built from.
    pub revision: u64,
    /// Number of moves.
    pub moves: usize,
    /// Job layers in run order.
    pub layers: Vec<libreburn_core::Layer>,
    /// Bounds of all moves in mm.
    pub bounds: Option<Bounds>,
    /// Run time estimate.
    pub estimate: libreburn_core::Estimate,
    /// Limit checks.
    pub warnings: Vec<libreburn_core::Warning>,
    /// Whether a hard limit blocks the job.
    pub blocked: bool,
}

fn plan_summary(plan: &Plan, revision: u64) -> PlanSummary {
    PlanSummary {
        revision,
        moves: plan.job.moves.len(),
        layers: plan.job.layers.clone(),
        bounds: plan
            .job
            .bounds()
            .map(|(min_x, min_y, max_x, max_y)| Bounds {
                min_x,
                min_y,
                max_x,
                max_y,
            }),
        estimate: plan.estimate.clone(),
        warnings: plan.warnings.clone(),
        blocked: plan.is_blocked(),
    }
}

async fn plan_job(State(s): State<AppState>, body: Bytes) -> ApiResult<Json<PlanSummary>> {
    let opts: PlanOptions = if body.is_empty() {
        PlanOptions::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| {
            api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Could not read plan options",
                e.to_string(),
                "send {} for defaults",
            )
        })?
    };
    let (project, profile, revision) = {
        let inner = s.lock();
        (
            inner.project.clone(),
            inner.profile.clone().ok_or_else(no_profile)?,
            inner.revision,
        )
    };
    let plan = tokio::task::spawn_blocking(move || libreburn_core::plan(&project, &profile, &opts))
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Planning crashed",
                e.to_string(),
                "report this",
            )
        })??;
    let summary = plan_summary(&plan, revision);
    let mut inner = s.lock();
    if inner.revision == revision {
        inner.plan = Some(plan);
        let _ = s.events.send(Event::PlanReady {
            revision,
            summary: Box::new(summary.clone()),
        });
    }
    Ok(Json(summary))
}

fn with_plan<T>(s: &AppState, f: impl FnOnce(&Plan, u64) -> T) -> ApiResult<T> {
    let inner = s.lock();
    inner
        .plan
        .as_ref()
        .map(|p| f(p, inner.revision))
        .ok_or_else(|| {
            api_error(
                StatusCode::CONFLICT,
                "No current plan",
                "nothing has been planned since the project last changed",
                "POST /api/v1/job/plan first",
            )
        })
}

async fn get_plan(State(s): State<AppState>) -> ApiResult<Json<PlanSummary>> {
    with_plan(&s, plan_summary).map(Json)
}

async fn get_moves(State(s): State<AppState>) -> ApiResult<Json<libreburn_core::Job>> {
    with_plan(&s, |p, _| p.job.clone()).map(Json)
}

/// Bytes per move in `/api/v1/job/moves.bin`.
pub const BINARY_MOVE_SIZE: usize = 15;

/// Pack moves for large previews: per move, little-endian `kind: u8`
/// (0 travel, 1 burn), `layer: u16`, `x: f32`, `y: f32`, `power_pct: f32`
/// (NaN when the move uses its layer's power).
pub fn pack_moves(job: &libreburn_core::Job) -> Vec<u8> {
    let mut out = Vec::with_capacity(job.moves.len() * BINARY_MOVE_SIZE);
    for m in &job.moves {
        out.push(u8::from(m.kind == libreburn_core::MoveKind::Burn));
        out.extend_from_slice(&u16::try_from(m.layer).unwrap_or(u16::MAX).to_le_bytes());
        out.extend_from_slice(&(m.x as f32).to_le_bytes());
        out.extend_from_slice(&(m.y as f32).to_le_bytes());
        out.extend_from_slice(&m.power_pct.map_or(f32::NAN, |p| p as f32).to_le_bytes());
    }
    out
}

async fn get_moves_bin(State(s): State<AppState>) -> ApiResult<Response> {
    let bytes = with_plan(&s, |p, _| pack_moves(&p.job))?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

#[derive(Debug, Deserialize)]
struct FrameQuery {
    #[serde(default)]
    shape: FrameShape,
}

async fn get_frame(
    State(s): State<AppState>,
    Query(q): Query<FrameQuery>,
) -> ApiResult<Json<Value>> {
    with_plan(
        &s,
        |p, _| serde_json::json!({ "outline": frame_outline(&p.job, q.shape) }),
    )
    .map(Json)
}

// ----------------------------------------------------------------- events

async fn events(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let rx = s.events.subscribe();
    let hello = {
        let inner = s.lock();
        Event::ProjectChanged {
            revision: inner.revision,
            reason: "connected".into(),
        }
    };
    ws.on_upgrade(move |socket| stream_events(socket, rx, hello))
}

async fn stream_events(mut socket: WebSocket, mut rx: broadcast::Receiver<Event>, hello: Event) {
    let send = |e: &Event| Message::Text(serde_json::to_string(e).unwrap_or_default().into());
    if socket.send(send(&hello)).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => if socket.send(send(&ev)).await.is_err() { return },
                // A slow client missed events; tell it to resync from scratch.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let msg = Message::Text(r#"{"type":"resync"}"#.into());
                    if socket.send(msg).await.is_err() { return }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}
