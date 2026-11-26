//! `inki serve`: a local web app. You upload an Anki export (and, to update
//! a deck, its `.inki` file from the reader) and download deck files.
//! Nothing is read from or written to your files.
//!
//! Listens on 127.0.0.1 only. Requests must carry a localhost Host header
//! (blocks DNS rebinding) and any Origin header must be this app's own, so
//! other websites cannot use it.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use axum::extract::{DefaultBodyLimit, Multipart, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use fsrs::FSRS;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::collection::Collection;
use crate::deck_export::{ExportOptions, build_decks, collect_cards, file_stem};
use crate::deck_file::{self, DeckFile};
use crate::revlog::training_set;
use crate::settings::{self, DeckSettings, SettingsSummary, StudySettings};
#[cfg(test)]
use crate::settings::{NewCardOrder, NewCardPlacement};
use crate::train::{TrainResult, train};
use crate::update::{UpdateSummary, update_deck};

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");
/// Anki packages with media can be large.
const MAX_UPLOAD: usize = 2 * 1024 * 1024 * 1024;
const KEEP_UPLOADS: usize = 3;
const KEEP_DOWNLOADS: usize = 10;

struct Upload {
    id: u64,
    file: NamedTempFile,
    training: Option<TrainResult>,
}

struct Download {
    id: u64,
    filename: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub struct AppState {
    uploads: Mutex<VecDeque<Upload>>,
    downloads: Mutex<VecDeque<Download>>,
    next_id: Mutex<u64>,
    /// Held while a long task (training, building a deck) runs.
    busy: tokio::sync::Mutex<()>,
}

type Shared = Arc<AppState>;

impl AppState {
    fn new_id(&self) -> u64 {
        let mut next = self.next_id.lock().expect("id lock");
        *next += 1;
        *next
    }

    fn add_upload(&self, file: NamedTempFile) -> u64 {
        let id = self.new_id();
        let mut uploads = self.uploads.lock().expect("uploads lock");
        uploads.push_back(Upload {
            id,
            file,
            training: None,
        });
        while uploads.len() > KEEP_UPLOADS {
            uploads.pop_front();
        }
        id
    }

    /// A private copy of an upload, so work can run without holding the lock
    /// and survives the upload being replaced meanwhile.
    fn upload_copy(&self, id: u64) -> Result<tempfile::TempPath> {
        let uploads = self.uploads.lock().expect("uploads lock");
        let upload = uploads
            .iter()
            .find(|u| u.id == id)
            .context("That Anki export is no longer loaded. Upload it again.")?;
        let copy = NamedTempFile::new().context("cannot create a temporary file")?;
        std::fs::copy(upload.file.path(), copy.path()).context("cannot copy the upload")?;
        Ok(copy.into_temp_path())
    }

    fn training(&self, id: u64) -> Option<TrainResult> {
        self.uploads
            .lock()
            .expect("uploads lock")
            .iter()
            .find(|u| u.id == id)
            .and_then(|u| u.training.clone())
    }

    fn add_download(&self, filename: String, bytes: Vec<u8>) -> String {
        let id = self.new_id();
        let mut downloads = self.downloads.lock().expect("downloads lock");
        downloads.push_back(Download {
            id,
            filename,
            bytes,
        });
        while downloads.len() > KEEP_DOWNLOADS {
            downloads.pop_front();
        }
        format!("/api/download/{id}")
    }
}

/// An error shown to the user as `{"error": "..."}`.
struct ApiError(StatusCode, String);

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self(StatusCode::BAD_REQUEST, format!("{err:#}"))
    }
}

impl From<axum::extract::multipart::MultipartError> for ApiError {
    fn from(err: axum::extract::multipart::MultipartError) -> Self {
        Self(err.status(), err.body_text())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn router(state: Shared) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", INDEX_HTML) }),
        )
        .route(
            "/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }),
        )
        .route(
            "/style.css",
            get(|| async { asset("text/css; charset=utf-8", STYLE_CSS) }),
        )
        .route("/api/anki", post(post_anki))
        .route("/api/deck", post(post_deck))
        .route("/api/inspect", post(post_inspect))
        .route("/api/update", post(post_update))
        .route("/api/train", post(post_train))
        .route("/api/download/{id}", get(get_download))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
        .layer(middleware::from_fn(same_site_only))
        .with_state(state)
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn is_local_host(host: &str) -> bool {
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

async fn same_site_only(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    // Browsers send Origin on cross-site POSTs, including file-upload forms,
    // which need no CORS approval. Only this page's own origin may post.
    let origin_ok = headers.get(header::ORIGIN).is_none_or(|origin| {
        origin
            .to_str()
            .ok()
            .and_then(|o| o.strip_prefix("http://"))
            .is_some_and(|o| o == host)
    });
    if !is_local_host(host) || !origin_ok {
        return (StatusCode::FORBIDDEN, "inki only answers its own page").into_response();
    }
    next.run(request).await
}

/// Runs blocking work (SQLite, training) off the async runtime, one task at a time.
async fn run_task<T: Send + 'static>(
    state: &Shared,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    let Ok(_guard) = state.busy.try_lock() else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Another task is still running.".into(),
        ));
    };
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(Into::into)
}

#[derive(Serialize)]
struct AnkiDeck {
    name: String,
    cards: usize,
    new: usize,
    suspended: usize,
}

#[derive(Serialize)]
struct AnkiUpload {
    id: u64,
    name: String,
    decks: Vec<AnkiDeck>,
    /// Reviews usable for training.
    reviews: usize,
}

fn describe_anki(path: &std::path::Path) -> Result<(Vec<AnkiDeck>, usize)> {
    let collection = Collection::open(path)?;
    let conn = collection.conn();
    let names = crate::anki_model::deck_names(conn)?;
    let mut decks: Vec<AnkiDeck> = names
        .values()
        .map(|name| AnkiDeck {
            name: name.clone(),
            cards: 0,
            new: 0,
            suspended: 0,
        })
        .collect();
    decks.sort_by(|a, b| a.name.cmp(&b.name));
    for card in collect_cards(conn, None)? {
        let Some(deck_name) = names.get(&card.row.deck_id) else {
            continue;
        };
        // Count the card in its deck and every parent deck.
        for end in deck_name
            .match_indices("::")
            .map(|(i, _)| i)
            .chain([deck_name.len()])
        {
            if let Some(deck) = decks.iter_mut().find(|d| d.name == deck_name[..end]) {
                deck.cards += 1;
                if card.row.queue == -1 {
                    deck.suspended += 1;
                } else if card.row.card_type == 0 {
                    deck.new += 1;
                }
            }
        }
    }
    decks.retain(|d| d.cards > 0);
    let reviews = training_set(collection.revlog()?).reviews;
    Ok((decks, reviews))
}

async fn post_anki(State(state): State<Shared>, mut multipart: Multipart) -> ApiResult<AnkiUpload> {
    let mut field = multipart
        .next_field()
        .await?
        .context("no file was uploaded")?;
    let name = field.file_name().unwrap_or("upload").to_string();
    let mut file = NamedTempFile::new().context("cannot create a temporary file")?;
    // Streamed to disk: packages with media can be far larger than memory allows.
    while let Some(chunk) = field.chunk().await? {
        file.write_all(&chunk).context("cannot store the upload")?;
    }
    let path = file.path().to_path_buf();
    let (decks, reviews) = run_task(&state, move || describe_anki(&path)).await?;
    let id = state.add_upload(file);
    Ok(Json(AnkiUpload {
        id,
        name,
        decks,
        reviews,
    }))
}

/// Which FSRS-7 parameters a deck gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ParamsChoice {
    /// The deck's own; for a new deck, the defaults.
    #[default]
    Keep,
    Default,
    /// Trained on the uploaded Anki export with `/api/train`.
    Trained,
}

impl ParamsChoice {
    fn parse(text: &str) -> Result<Self> {
        match text.trim() {
            "" | "keep" => Ok(Self::Keep),
            "default" => Ok(Self::Default),
            "trained" => Ok(Self::Trained),
            other => Err(anyhow!("unknown parameter choice {other:?}")),
        }
    }

    fn apply(self, state: &AppState, anki: Option<u64>, settings: &mut DeckSettings) -> Result<()> {
        match self {
            Self::Keep => {}
            Self::Default => settings.params = DeckSettings::default().params,
            Self::Trained => {
                let training = anki
                    .and_then(|id| state.training(id))
                    .context("Train on your reviews first.")?;
                settings::set_params(settings, &training.params)?;
            }
        }
        Ok(())
    }
}

fn export_options<'a>(
    fsrs: &'a FSRS,
    settings: &DeckSettings,
    deck: Option<&'a str>,
) -> ExportOptions<'a> {
    ExportOptions {
        deck,
        with_progress: true,
        fsrs,
        desired_retention: settings.desired_retention,
    }
}

#[derive(Serialize)]
struct DownloadReady<S> {
    url: String,
    filename: String,
    summary: S,
}

#[derive(Deserialize)]
struct DeckRequest {
    anki: u64,
    deck: String,
    #[serde(default)]
    settings: StudySettings,
    #[serde(default)]
    params: ParamsChoice,
}

#[derive(Serialize)]
struct NewDeckSummary {
    anki_deck: String,
    cards: usize,
    studied: usize,
    suspended: usize,
    settings: SettingsSummary,
}

async fn post_deck(
    State(state): State<Shared>,
    Json(request): Json<DeckRequest>,
) -> ApiResult<DownloadReady<NewDeckSummary>> {
    let path = state.upload_copy(request.anki)?;
    let mut deck_settings = DeckSettings::default();
    request.settings.check()?.apply_to(&mut deck_settings);
    request
        .params
        .apply(&state, Some(request.anki), &mut deck_settings)?;
    let (filename, bytes, summary) = run_task(&state, move || {
        let fsrs = settings::fsrs(&deck_settings)?;
        let collection = Collection::open(&path)?;
        let deck = build_decks(
            &collection,
            &export_options(&fsrs, &deck_settings, Some(&request.deck)),
        )?
        .into_iter()
        .next()
        .context("that deck has no cards")?;
        let file = DeckFile {
            anki_deck: deck.name.clone(),
            settings: deck_settings,
            cards: deck.cards.clone(),
            answer_log: Vec::new(),
        };
        Ok((
            format!("{}.{}", file_stem(&deck.name), deck_file::EXTENSION),
            file.encode()?,
            NewDeckSummary {
                anki_deck: deck.name.clone(),
                cards: deck.cards.len(),
                studied: deck.studied(),
                suspended: deck.suspended,
                settings: SettingsSummary::of(&deck_settings),
            },
        ))
    })
    .await?;
    let url = state.add_download(filename.clone(), bytes);
    Ok(Json(DownloadReady {
        url,
        filename,
        summary,
    }))
}

#[derive(Serialize)]
struct DeckInfo {
    anki_deck: String,
    cards: usize,
    studied: usize,
    answers: usize,
    settings: SettingsSummary,
}

/// Reads a deck file (multipart field `deck`) to show what it holds.
async fn post_inspect(mut multipart: Multipart) -> ApiResult<DeckInfo> {
    let mut deck = None;
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("deck") {
            let name = field.file_name().unwrap_or("deck.inki").to_string();
            deck = Some((name, field.bytes().await?));
        }
    }
    let (name, bytes) =
        deck.ok_or_else(|| anyhow!("Choose the deck's .inki file from the reader."))?;
    let deck =
        DeckFile::decode(&bytes).with_context(|| format!("{name} is not an inki deck file"))?;
    Ok(Json(DeckInfo {
        anki_deck: deck.anki_deck.clone(),
        cards: deck.cards.len(),
        studied: deck.studied(),
        answers: deck.answers(),
        settings: SettingsSummary::of(&deck.settings),
    }))
}

#[derive(Serialize)]
struct UpdateResult {
    anki_deck: String,
    cards: usize,
    /// What the Anki export changed; absent when only settings changed.
    from_anki: Option<UpdateSummary>,
    settings: SettingsSummary,
}

/// Multipart fields: `deck` (the .inki file); optionally `anki` (upload id)
/// to add new cards, `settings` (JSON study settings) and `params`
/// (keep, default or trained).
async fn post_update(
    State(state): State<Shared>,
    mut multipart: Multipart,
) -> ApiResult<DownloadReady<UpdateResult>> {
    let mut anki = None;
    let mut deck = None;
    let mut study = None;
    let mut params = ParamsChoice::Keep;
    while let Some(field) = multipart.next_field().await? {
        match field.name() {
            Some("anki") => anki = field.text().await?.trim().parse::<u64>().ok(),
            Some("deck") => {
                let name = field.file_name().unwrap_or("deck.inki").to_string();
                deck = Some((name, field.bytes().await?));
            }
            Some("settings") => {
                let text = field.text().await?;
                let parsed: StudySettings =
                    serde_json::from_str(&text).context("the settings are not valid")?;
                study = Some(parsed.check()?);
            }
            Some("params") => params = ParamsChoice::parse(&field.text().await?)?,
            _ => {}
        }
    }
    let (deck_name, deck_bytes) =
        deck.ok_or_else(|| anyhow!("Choose the deck's .inki file from the reader."))?;
    let mut on_reader = DeckFile::decode(&deck_bytes)
        .with_context(|| format!("{deck_name} is not an inki deck file"))?;
    if let Some(study) = study {
        study.apply_to(&mut on_reader.settings);
    }
    params.apply(&state, anki, &mut on_reader.settings)?;
    let path = anki.map(|id| state.upload_copy(id)).transpose()?;
    let (filename, bytes, summary) = run_task(&state, move || {
        let (updated, from_anki) = match path {
            Some(path) => {
                let stem = deck_name
                    .strip_suffix(&format!(".{}", deck_file::EXTENSION))
                    .unwrap_or(&deck_name)
                    .to_string();
                let fsrs = settings::fsrs(&on_reader.settings)?;
                let collection = Collection::open(&path)?;
                let (updated, summary) = update_deck(
                    &collection,
                    &stem,
                    &on_reader,
                    &export_options(&fsrs, &on_reader.settings, None),
                )?;
                (updated, Some(summary))
            }
            None => (on_reader, None),
        };
        let summary = UpdateResult {
            anki_deck: updated.anki_deck.clone(),
            cards: updated.cards.len(),
            from_anki,
            settings: SettingsSummary::of(&updated.settings),
        };
        // Same name as the reader's file, so copying it back replaces it.
        Ok((deck_name, updated.encode()?, summary))
    })
    .await?;
    let url = state.add_download(filename.clone(), bytes);
    Ok(Json(DownloadReady {
        url,
        filename,
        summary,
    }))
}

#[derive(Deserialize)]
struct TrainRequest {
    anki: u64,
}

async fn post_train(
    State(state): State<Shared>,
    Json(request): Json<TrainRequest>,
) -> ApiResult<TrainResult> {
    let path = state.upload_copy(request.anki)?;
    let result = run_task(&state, move || train(&path)).await?;
    if let Some(upload) = state
        .uploads
        .lock()
        .expect("uploads lock")
        .iter_mut()
        .find(|u| u.id == request.anki)
    {
        upload.training = Some(result.clone());
    }
    Ok(Json(result))
}

/// Content-Disposition with a UTF-8 `filename*`, so any deck name survives.
fn content_disposition(filename: &str) -> HeaderValue {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .-_()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = filename
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b".-_".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

async fn get_download(State(state): State<Shared>, Path(id): Path<u64>) -> Response {
    let downloads = state.downloads.lock().expect("downloads lock");
    let Some(download) = downloads.iter().find(|d| d.id == id) else {
        return (StatusCode::NOT_FOUND, "This download has expired.").into_response();
    };
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (
                header::CONTENT_DISPOSITION,
                content_disposition(&download.filename),
            ),
        ],
        download.bytes.clone(),
    )
        .into_response()
}

/// Serves the app until the process is stopped.
pub async fn serve(port: u16) -> Result<()> {
    let state = Arc::new(AppState::default());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("cannot listen on port {port}"))?;
    eprintln!("inki is running at http://localhost:{port}  (Ctrl+C to stop)");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck_export::CardState;
    use crate::deck_export::tests::current_schema;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const BOUNDARY: &str = "inki-test-boundary";

    fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (name, filename, bytes) in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            let disposition = match filename {
                Some(f) => format!("form-data; name=\"{name}\"; filename=\"{f}\""),
                None => format!("form-data; name=\"{name}\""),
            };
            body.extend_from_slice(
                format!("Content-Disposition: {disposition}\r\n\r\n").as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    struct Reply {
        status: StatusCode,
        bytes: Vec<u8>,
        disposition: Option<String>,
    }

    impl Reply {
        fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.bytes).unwrap_or(serde_json::Value::Null)
        }
    }

    async fn send(app: &Router, request: axum::http::Request<Body>) -> Reply {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        Reply {
            status,
            bytes,
            disposition,
        }
    }

    fn request(method: &str, path: &str) -> axum::http::request::Builder {
        axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "localhost:8787")
    }

    async fn post_json(app: &Router, path: &str, json: serde_json::Value) -> Reply {
        let req = request("POST", path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap();
        send(app, req).await
    }

    async fn post_multipart(
        app: &Router,
        path: &str,
        parts: &[(&str, Option<&str>, &[u8])],
    ) -> Reply {
        let req = request("POST", path)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(multipart(parts)))
            .unwrap();
        send(app, req).await
    }

    async fn download(app: &Router, ready: &serde_json::Value) -> Reply {
        let url = ready["url"].as_str().unwrap();
        send(app, request("GET", url).body(Body::empty()).unwrap()).await
    }

    async fn upload_collection(app: &Router, path: &std::path::Path) -> u64 {
        let bytes = std::fs::read(path).unwrap();
        let reply = post_multipart(
            app,
            "/api/anki",
            &[("file", Some("collection.anki2"), &bytes)],
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
        reply.json()["id"].as_u64().unwrap()
    }

    #[tokio::test]
    async fn refuses_other_hosts_and_origins() {
        let app = router(Arc::new(AppState::default()));
        let other_host = axum::http::Request::builder()
            .uri("/")
            .header(header::HOST, "evil.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&app, other_host).await.status, StatusCode::FORBIDDEN);
        let deck_request = |origin: &str| {
            request("POST", "/api/deck")
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"anki": 1, "deck": "x"}"#))
                .unwrap()
        };
        let other_origin = send(&app, deck_request("https://evil.example")).await;
        assert_eq!(other_origin.status, StatusCode::FORBIDDEN);
        // Reaches the handler, which finds no such upload.
        let own_origin = send(&app, deck_request("http://localhost:8787")).await;
        assert_eq!(own_origin.status, StatusCode::BAD_REQUEST);
        let page = request("GET", "/").body(Body::empty()).unwrap();
        assert_eq!(send(&app, page).await.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn make_study_and_update_a_deck() {
        let dir = tempfile::tempdir().unwrap();
        let collection = dir.path().join("collection.anki2");
        current_schema(&collection);
        let app = router(Arc::new(AppState::default()));

        let reply = post_multipart(
            &app,
            "/api/anki",
            &[(
                "file",
                Some("collection.anki2"),
                &std::fs::read(&collection).unwrap(),
            )],
        )
        .await;
        let upload = reply.json();
        let names: Vec<&str> = upload["decks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Languages", "Languages::German", "Science"]);
        assert_eq!(upload["reviews"], 2);
        let anki = upload["id"].as_u64().unwrap();

        let ready = post_json(
            &app,
            "/api/deck",
            serde_json::json!({
                "anki": anki,
                "deck": "Languages",
                "settings": {
                    "new_per_day": 5,
                    "reviews_per_day": 100,
                    "desired_retention": 0.85,
                    "new_card_order": "random",
                },
                "params": "default",
            }),
        )
        .await;
        assert_eq!(ready.status, StatusCode::OK, "{:?}", ready.json());
        let ready = ready.json();
        assert_eq!(ready["filename"], "Languages.inki");
        assert_eq!(ready["summary"]["cards"], 3);
        assert_eq!(ready["summary"]["settings"]["new_per_day"], 5);
        assert_eq!(ready["summary"]["settings"]["trained_params"], false);
        let file = download(&app, &ready).await;
        assert!(file.disposition.unwrap().contains("Languages.inki"));
        let mut deck = DeckFile::decode(&file.bytes).unwrap();
        assert_eq!(
            StudySettings::of(&deck.settings),
            StudySettings {
                new_per_day: 5,
                reviews_per_day: 100,
                desired_retention: 0.85,
                new_card_order: NewCardOrder::Random,
                new_card_placement: NewCardPlacement::AfterReviews,
            }
        );

        // Study a card on the reader, then add a card in Anki and re-export.
        deck.cards[2].state = CardState {
            stability: 4.0,
            difficulty: 5.0,
            stability_fast: 3.0,
            last_review: 1_800_000_000,
            due: 1_800_300_000,
        };
        deck.answer_log = vec![3; 32];
        rusqlite::Connection::open(&collection)
            .unwrap()
            .execute_batch(
                "INSERT INTO notes VALUES (4, 'd', 100, 0, 0, '', 'Maus' || char(31) || 'mouse', '', 0, 0, '');
                 INSERT INTO cards VALUES (41, 4, 2, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, '');",
            )
            .unwrap();
        let anki = upload_collection(&app, &collection).await;

        let reply = post_multipart(
            &app,
            "/api/update",
            &[
                ("anki", None, anki.to_string().as_bytes()),
                ("deck", Some("Languages.inki"), &deck.encode().unwrap()),
            ],
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
        let ready = reply.json();
        assert_eq!(ready["filename"], "Languages.inki");
        assert_eq!(ready["summary"]["from_anki"]["new_cards"], 1);
        assert_eq!(ready["summary"]["from_anki"]["studied_on_reader"], 2);
        let updated = DeckFile::decode(&download(&app, &ready).await.bytes).unwrap();
        assert_eq!(updated.settings, deck.settings, "settings kept");
        assert_eq!(updated.cards.len(), 4);
        assert_eq!(updated.cards[2].state, deck.cards[2].state);
        assert_eq!(updated.answer_log, deck.answer_log);
        assert_eq!(updated.cards[3].front, "Maus");
    }

    #[tokio::test]
    async fn update_rejects_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let collection = dir.path().join("collection.anki2");
        current_schema(&collection);
        let app = router(Arc::new(AppState::default()));
        let anki = upload_collection(&app, &collection).await;
        let reply = post_multipart(
            &app,
            "/api/update",
            &[
                ("anki", None, anki.to_string().as_bytes()),
                ("deck", Some("notes.txt"), b"hello"),
            ],
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert!(
            reply.json()["error"]
                .as_str()
                .unwrap()
                .contains("not an inki deck file")
        );
        let reply = post_json(
            &app,
            "/api/deck",
            serde_json::json!({ "anki": 999, "deck": "x" }),
        )
        .await;
        assert!(
            reply.json()["error"]
                .as_str()
                .unwrap()
                .contains("Upload it again")
        );
    }

    #[tokio::test]
    async fn changes_settings_without_an_anki_export() {
        let mut settings = DeckSettings::default();
        settings.params[0] = 0.5;
        let deck = DeckFile {
            anki_deck: "Science".into(),
            settings,
            cards: Vec::new(),
            answer_log: vec![1; 32],
        };
        let bytes = deck.encode().unwrap();
        let app = router(Arc::new(AppState::default()));
        let study = br#"{"new_per_day": 0, "reviews_per_day": 50, "desired_retention": 0.8}"#;

        let reply = post_multipart(
            &app,
            "/api/update",
            &[
                ("deck", Some("Science.inki"), &bytes),
                ("settings", None, study),
            ],
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
        let ready = reply.json();
        assert!(ready["summary"]["from_anki"].is_null());
        assert_eq!(ready["summary"]["settings"]["trained_params"], true);
        let updated = DeckFile::decode(&download(&app, &ready).await.bytes).unwrap();
        assert_eq!(updated.settings.new_per_day, 0);
        assert_eq!(
            updated.settings.new_card_placement,
            inki_core::format::NEW_AFTER_REVIEWS,
            "options missing from the request get their defaults"
        );
        let info = post_multipart(
            &app,
            "/api/inspect",
            &[("deck", Some("Science.inki"), &updated.encode().unwrap())],
        )
        .await
        .json();
        assert_eq!(info["anki_deck"], "Science");
        assert_eq!(info["answers"], 1);
        assert_eq!(info["settings"]["reviews_per_day"], 50);
        assert_eq!(updated.settings.desired_retention, 0.8);
        assert_eq!(updated.settings.params, settings.params, "parameters kept");
        assert_eq!(updated.answer_log, deck.answer_log);

        let reply = post_multipart(
            &app,
            "/api/update",
            &[
                ("deck", Some("Science.inki"), &bytes),
                ("params", None, b"default"),
            ],
        )
        .await;
        let updated = DeckFile::decode(&download(&app, &reply.json()).await.bytes).unwrap();
        assert!(updated.settings.uses_default_params());
        assert_eq!(updated.settings.new_per_day, 20, "limits kept");

        let reply = post_multipart(
            &app,
            "/api/update",
            &[
                ("deck", Some("Science.inki"), &bytes),
                ("params", None, b"trained"),
            ],
        )
        .await;
        assert!(
            reply.json()["error"]
                .as_str()
                .unwrap()
                .contains("Train on your reviews")
        );
        let bad = br#"{"new_per_day": 0, "reviews_per_day": 50, "desired_retention": 1.5}"#;
        let reply = post_multipart(
            &app,
            "/api/update",
            &[
                ("deck", Some("Science.inki"), &bytes),
                ("settings", None, bad),
            ],
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn content_disposition_keeps_unicode_names() {
        let value = content_disposition("Español \"1\".inki");
        let text = value.to_str().unwrap();
        assert!(text.contains("filename=\"Espa_ol _1_.inki\""), "{text}");
        assert!(
            text.contains("filename*=UTF-8''Espa%C3%B1ol%20%221%22.inki"),
            "{text}"
        );
    }
}
