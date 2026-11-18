use reqwest::{Client, Response, Url, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncReadExt;

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Citation {
    pub file_name: String,
    pub source: String,
    pub page_number: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Answer {
    pub text: String,
    pub citations: Vec<Citation>,
}

#[derive(Serialize)]
pub struct DirectoryUpload {
    pub total: usize,
    pub files: Vec<DirectoryUploadFile>,
    pub error: Option<String>,
    pub timed_out: bool,
}

#[derive(Serialize)]
pub struct DirectoryUploadFile {
    pub path: String,
    pub operation: Option<Value>,
}

// Keep API keys, file content, URLs, and provider error bodies out of error messages.
#[derive(Debug)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;
fn invalid(message: &str) -> Error {
    Error(message.into())
}

#[derive(Clone)]
pub struct Gemini {
    client: Client,
    key: HeaderValue,
    base: Url,
    root: PathBuf,
}

impl Gemini {
    pub fn new(key: &str, root: &Path) -> Result<Self> {
        Self::with_base(
            key,
            root,
            Url::parse("https://generativelanguage.googleapis.com/").unwrap(),
        )
    }

    fn with_base(key: &str, root: &Path, base: Url) -> Result<Self> {
        if key.trim().is_empty() {
            return Err(invalid("Set GEMINI_FILE_SEARCH_API_KEY."));
        }
        let mut key =
            HeaderValue::from_str(key).map_err(|_| invalid("The API key format is incorrect."))?;
        key.set_sensitive(true);
        let root = root
            .canonicalize()
            .map_err(|_| invalid("GEMINI_UPLOAD_ROOT must be an existing directory."))?;
        if !root.is_dir() {
            return Err(invalid("GEMINI_UPLOAD_ROOT must be a directory."));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| invalid("The app cannot start the HTTPS client."))?;
        Ok(Self {
            client,
            key,
            base,
            root,
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        self.base.join(path).expect("validated API path")
    }

    pub async fn create_store(&self, display_name: &str) -> Result<Value> {
        if display_name.trim().is_empty() || display_name.chars().count() > 512 {
            return Err(invalid("display_name must contain 1 to 512 characters."));
        }
        let response = self
            .client
            .post(self.endpoint("v1beta/fileSearchStores"))
            .header("x-goog-api-key", self.key.clone())
            .json(&json!({"displayName": display_name}))
            .send()
            .await
            .map_err(network)?;
        json_response(response).await
    }

    pub async fn list_stores(&self, page_size: u8, page_token: Option<&str>) -> Result<Value> {
        if !(1..=20).contains(&page_size) {
            return Err(invalid("page_size must be between 1 and 20."));
        }
        if page_token.is_some_and(|token| token.len() > 4096) {
            return Err(invalid("page_token exceeds 4096 bytes."));
        }
        let mut request = self
            .client
            .get(self.endpoint("v1beta/fileSearchStores"))
            .header("x-goog-api-key", self.key.clone())
            .query(&[("pageSize", page_size.to_string())]);
        if let Some(token) = page_token {
            request = request.query(&[("pageToken", token)]);
        }
        json_response(request.send().await.map_err(network)?).await
    }

    pub async fn get_operation(&self, name: &str) -> Result<Value> {
        validate_operation(name)?;
        json_response(
            self.client
                .get(self.endpoint(&format!("v1beta/{name}")))
                .header("x-goog-api-key", self.key.clone())
                .send()
                .await
                .map_err(network)?,
        )
        .await
    }

    pub async fn list_documents(&self, store: &str, page_token: Option<&str>) -> Result<Value> {
        validate_store(store)?;
        if page_token.is_some_and(|token| token.len() > 4096) {
            return Err(invalid("page_token exceeds 4096 bytes."));
        }
        let mut request = self
            .client
            .get(self.endpoint(&format!("v1beta/{store}/documents")))
            .header("x-goog-api-key", self.key.clone())
            .query(&[("pageSize", "20")]);
        if let Some(token) = page_token {
            request = request.query(&[("pageToken", token)]);
        }
        json_response(request.send().await.map_err(network)?).await
    }

    pub async fn delete_document(&self, name: &str) -> Result<Value> {
        validate_document(name)?;
        json_response(
            self.client
                .delete(self.endpoint(&format!("v1beta/{name}")))
                .header("x-goog-api-key", self.key.clone())
                .query(&[("force", "true")])
                .send()
                .await
                .map_err(network)?,
        )
        .await
    }

    pub async fn delete_store(&self, name: &str) -> Result<Value> {
        validate_store(name)?;
        json_response(
            self.client
                .delete(self.endpoint(&format!("v1beta/{name}")))
                .header("x-goog-api-key", self.key.clone())
                .query(&[("force", "true")])
                .send()
                .await
                .map_err(network)?,
        )
        .await
    }

    pub async fn ask(&self, store: &str, question: &str, model: &str) -> Result<Answer> {
        validate_store(store)?;
        if question.trim().is_empty() || question.len() > 32_000 {
            return Err(invalid("The question must contain 1 to 32000 bytes."));
        }
        if model.trim().is_empty() || model.len() > 128 {
            return Err(invalid("The model name must contain 1 to 128 bytes."));
        }
        let response = self
            .client
            .post(self.endpoint("v1beta/interactions"))
            .header("x-goog-api-key", self.key.clone())
            .json(&json!({
                "model": model,
                "input": question,
                "tools": [{"type": "file_search", "file_search_store_names": [store]}]
            }))
            .send()
            .await
            .map_err(network)?;
        parse_answer(&json_response(response).await?)
    }

    /// Return the last operation if the time limit expires. Keep its operation ID.
    /// Send only GET requests again. Do not upload the file again.
    pub async fn wait_for_operation(&self, mut operation: Value, seconds: u64) -> Result<Value> {
        let name = operation
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("The response does not include the operation name."))?
            .to_owned();
        validate_operation(&name)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        while operation.get("done").and_then(Value::as_bool) != Some(true) {
            let poll = async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                self.get_operation(&name).await
            };
            match tokio::time::timeout_at(deadline, poll).await {
                Ok(result) => {
                    operation = result.map_err(|error| {
                        Error(format!(
                            "{error} Read the status of operation {name} again."
                        ))
                    })?
                }
                Err(_) => break,
            }
        }
        Ok(operation)
    }

    pub async fn upload_file(&self, store_name: &str, path: &str) -> Result<Value> {
        self.upload_local_file(store_name, Path::new(path)).await
    }

    /// Return None for a single-file target so callers can keep their existing output.
    pub async fn upload_directory(
        &self,
        store: &str,
        path: &str,
        wait: Option<u64>,
        mut progress: impl FnMut(usize, usize, Option<&str>),
    ) -> Result<Option<DirectoryUpload>> {
        validate_store(store)?;
        if path.is_empty() {
            return Err(invalid(
                "Enter a file or directory path. Use . for the upload root.",
            ));
        }
        let target = self.upload_path(Path::new(path)).await?;
        if !target.is_dir() {
            return Ok(None);
        }

        let mut paths = Vec::new();
        let mut directories = vec![target];
        while let Some(directory) = directories.pop() {
            let scan_error = |_| invalid("The app cannot read the upload directory.");
            let mut entries = tokio::fs::read_dir(directory).await.map_err(scan_error)?;
            while let Some(entry) = entries.next_entry().await.map_err(scan_error)? {
                let kind = entry.file_type().await.map_err(scan_error)?;
                let path = entry.path();
                if kind.is_dir() {
                    directories.push(path);
                } else if kind.is_file() && upload_mime(&path).is_some() {
                    let size = entry.metadata().await.map_err(scan_error)?.len();
                    if (1..=MAX_FILE_BYTES).contains(&size) {
                        paths.push(path);
                    }
                }
            }
        }
        if paths.is_empty() {
            return Err(invalid(
                "The directory has no eligible files. Use nonempty .md, .txt, or .pdf files up to 10 MiB.",
            ));
        }
        paths.sort();
        let mut report = DirectoryUpload {
            total: paths.len(),
            files: Vec::new(),
            error: None,
            timed_out: false,
        };
        for path in paths {
            let index = report.files.len() + 1;
            progress(index, report.total, None);
            let mut file = DirectoryUploadFile {
                path: path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned(),
                operation: None,
            };
            match self.upload_local_file(store, &path).await {
                Ok(operation) => file.operation = Some(operation),
                Err(error) => report.error = Some(error.to_string()),
            }
            if let (Some(seconds), Some(operation)) = (wait, &file.operation) {
                progress(index, report.total, operation["name"].as_str());
                match self.wait_for_operation(operation.clone(), seconds).await {
                    Ok(operation) => file.operation = Some(operation),
                    Err(error) => report.error = Some(error.to_string()),
                }
            }
            if let Some(operation) = &file.operation {
                if operation.get("error").is_some_and(|error| !error.is_null()) {
                    report.error = Some(
                        "Indexing failed. Examine the operation before uploading the file again."
                            .into(),
                    );
                } else if wait.is_some() && report.error.is_none() && operation["done"] != true {
                    report.timed_out = true;
                }
            }
            report.files.push(file);
            if report.error.is_some() || report.timed_out {
                break;
            }
        }
        Ok(Some(report))
    }

    async fn upload_path(&self, path: &Path) -> Result<PathBuf> {
        let candidate = tokio::fs::canonicalize(self.root.join(path))
            .await
            .map_err(|_| {
                invalid("The app cannot find the file or directory in GEMINI_UPLOAD_ROOT.")
            })?;
        if !candidate.starts_with(&self.root) {
            return Err(invalid(
                "The path must stay in GEMINI_UPLOAD_ROOT, including symbolic-link targets.",
            ));
        }
        Ok(candidate)
    }

    async fn upload_local_file(&self, store_name: &str, path: &Path) -> Result<Value> {
        validate_store(store_name)?;
        let candidate = self.upload_path(path).await?;
        let mime =
            upload_mime(&candidate).ok_or_else(|| invalid("Use a .md, .txt, or .pdf file."))?;
        let file = tokio::fs::File::open(&candidate)
            .await
            .map_err(|_| invalid("The app cannot open the file."))?;
        let metadata = file
            .metadata()
            .await
            .map_err(|_| invalid("The app cannot read the file metadata."))?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES {
            return Err(invalid(
                "Use a regular file that contains data. The maximum size is 10 MiB.",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| invalid("The app cannot read the file."))?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(invalid("The file size changed or exceeds 10 MiB."));
        }
        let display_name = candidate
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| invalid("The filename must use UTF-8."))?;
        let response = self
            .client
            .post(self.endpoint(&format!(
                "upload/v1beta/{store_name}:uploadToFileSearchStore"
            )))
            .header("x-goog-api-key", self.key.clone())
            .header("X-Goog-Upload-Protocol", "resumable")
            .header("X-Goog-Upload-Command", "start")
            .header("X-Goog-Upload-Header-Content-Length", bytes.len())
            .header("X-Goog-Upload-Header-Content-Type", mime)
            .json(&json!({"displayName": display_name, "mimeType": mime}))
            .send()
            .await
            .map_err(network)?;
        check_status(&response)?;
        let location = response
            .headers()
            .get("x-goog-upload-url")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                invalid("Google did not return an upload session URL. Examine the store before you upload the file again.")
            })?;
        let upload_url =
            Url::parse(location).map_err(|_| invalid("The upload session URL is incorrect."))?;
        // Send the key and file only to the Gemini API origin.
        if upload_url.origin() != self.base.origin()
            || !upload_url.username().is_empty()
            || upload_url.password().is_some()
        {
            return Err(invalid(
                "Upload session URL must use the Gemini API origin.",
            ));
        }
        let response = self
            .client
            .post(upload_url)
            .header("x-goog-api-key", self.key.clone())
            .header("X-Goog-Upload-Offset", "0")
            .header("X-Goog-Upload-Command", "upload, finalize")
            .header("Content-Type", mime)
            .body(bytes)
            .send()
            .await
            .map_err(network)?;
        let operation = json_response(response).await?;
        // Keep the operation name to read the indexing status.
        let name = operation
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                invalid("The upload returned no operation name. Examine the store before you upload the file again.")
            })?;
        validate_operation(name)?;
        Ok(operation)
    }
}

fn upload_mime(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("md") => Some("text/markdown"),
        Some("txt") => Some("text/plain"),
        Some("pdf") => Some("application/pdf"),
        _ => None,
    }
}

fn parse_answer(response: &Value) -> Result<Answer> {
    let mut text = Vec::new();
    let mut citations = Vec::new();
    if let Some(steps) = response.get("steps").and_then(Value::as_array) {
        for step in steps.iter().filter(|step| step["type"] == "model_output") {
            if let Some(content) = step.get("content").and_then(Value::as_array) {
                for block in content.iter().filter(|block| block["type"] == "text") {
                    if let Some(value) = block.get("text").and_then(Value::as_str) {
                        text.push(value.to_owned());
                    }
                    if let Some(annotations) = block.get("annotations").and_then(Value::as_array) {
                        for annotation in annotations
                            .iter()
                            .filter(|item| item["type"] == "file_citation")
                        {
                            let citation = Citation {
                                file_name: annotation
                                    .get("file_name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Source")
                                    .to_owned(),
                                source: annotation
                                    .get("source")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                page_number: annotation.get("page_number").and_then(Value::as_u64),
                            };
                            if !citations.contains(&citation) {
                                citations.push(citation);
                            }
                        }
                    }
                }
            }
        }
    }
    let text = text.join("\n\n");
    if text.trim().is_empty() {
        return Err(invalid(
            "Gemini did not return an answer. Examine the model support and document status.",
        ));
    }
    Ok(Answer { text, citations })
}

fn network(_: reqwest::Error) -> Error {
    invalid(
        "The Gemini request did not complete. Google can complete a write request without a response. Before you send the request again, examine the store.",
    )
}
fn check_status(response: &Response) -> Result<()> {
    if !response.status().is_success() {
        return Err(Error(format!(
            "Gemini returned HTTP {}. Examine the key, resource name, and quota. Examine the store before you send a write request again.",
            response.status().as_u16()
        )));
    }
    Ok(())
}
async fn json_response(response: Response) -> Result<Value> {
    check_status(&response)?;
    response.json().await.map_err(|_| {
        invalid(
            "Gemini returned invalid JSON. Examine the resource before you send a write request again.",
        )
    })
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
pub fn validate_store(name: &str) -> Result<()> {
    let id = name
        .strip_prefix("fileSearchStores/")
        .ok_or_else(|| invalid("Use a resource name in the fileSearchStores/<id> format."))?;
    if id.len() > 40
        || !valid_id(id)
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(invalid("The resource name for the store is incorrect."));
    }
    Ok(())
}
pub fn validate_operation(name: &str) -> Result<()> {
    let parts: Vec<_> = name.split('/').collect();
    let store = match parts.as_slice() {
        ["fileSearchStores", id, "upload", "operations", operation] if valid_id(operation) => *id,
        ["fileSearchStores", id, "operations", operation] if valid_id(operation) => *id,
        _ => {
            return Err(invalid(
                "Use the operation name that the upload tool returns.",
            ));
        }
    };
    validate_store(&format!("fileSearchStores/{store}"))
}

pub fn validate_document(name: &str) -> Result<()> {
    let parts: Vec<_> = name.split('/').collect();
    match parts.as_slice() {
        ["fileSearchStores", store, "documents", document] if valid_id(document) => {
            validate_store(&format!("fileSearchStores/{store}"))
        }
        _ => Err(invalid(
            "Use a resource name in the fileSearchStores/<id>/documents/<id> format.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::State,
        http::Request,
        response::Response,
        routing::any,
    };
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    #[derive(Clone)]
    struct Mock {
        origin: String,
        seen: Arc<Mutex<Vec<(String, Value)>>>,
        foreign_upload: bool,
    }
    async fn handler(State(state): State<Mock>, request: Request<Body>) -> Response {
        assert_eq!(
            request.headers().get("x-goog-api-key").unwrap(),
            "test-secret"
        );
        let path = request.uri().path().to_owned();
        let query = request.uri().query().unwrap_or("").to_owned();
        let method = request.method().clone();
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, 11 * 1024 * 1024).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        state
            .seen
            .lock()
            .unwrap()
            .push((format!("{method} {path}"), value.clone()));
        if query.contains("fail") {
            return Response::builder()
                .status(403)
                .body(Body::from("test-secret private provider content"))
                .unwrap();
        }
        if path.starts_with("/upload/") {
            assert_eq!(parts.headers["x-goog-upload-protocol"], "resumable");
            assert_eq!(
                parts.headers["x-goog-upload-header-content-type"],
                "text/markdown"
            );
            assert_eq!(value["displayName"], "guide.md");
            let location = if state.foreign_upload {
                "https://evil.example/steal".into()
            } else {
                format!("{}/session", state.origin)
            };
            return Response::builder()
                .header("x-goog-upload-url", location)
                .body(Body::empty())
                .unwrap();
        }
        let output = match (method.as_str(), path.as_str()) {
            ("POST", "/v1beta/fileSearchStores") => {
                assert_eq!(value["displayName"], "My new store");
                json!({"name":"fileSearchStores/test", "displayName":"My new store"})
            }
            ("GET", "/v1beta/fileSearchStores") => {
                assert!(query.contains("pageSize=2"));
                assert!(query.contains("pageToken=next"));
                json!({"fileSearchStores":[{"name":"fileSearchStores/test"}], "nextPageToken":"page-2"})
            }
            ("POST", "/session") => {
                assert_eq!(parts.headers["x-goog-upload-command"], "upload, finalize");
                assert_eq!(parts.headers["x-goog-upload-offset"], "0");
                assert_eq!(bytes.as_ref(), b"# Approved guide\n");
                json!({"name":"fileSearchStores/test/upload/operations/upload-1", "done":false})
            }
            ("GET", "/v1beta/fileSearchStores/test/upload/operations/upload-1") => {
                json!({"name":"fileSearchStores/test/upload/operations/upload-1", "done":true, "response":{}})
            }
            ("GET", "/v1beta/fileSearchStores/test/documents") => {
                assert!(query.contains("pageSize=20"));
                assert!(query.contains("pageToken=next"));
                json!({"documents":[{"name":"fileSearchStores/test/documents/guide", "displayName":"guide.md", "state":"STATE_ACTIVE"}], "nextPageToken":"page-2"})
            }
            (
                "DELETE",
                "/v1beta/fileSearchStores/test/documents/guide" | "/v1beta/fileSearchStores/test",
            ) => {
                assert_eq!(query, "force=true");
                json!({})
            }
            ("POST", "/v1beta/interactions") => {
                assert_eq!(value["model"], "test-model");
                assert_eq!(value["input"], "What does the guide say?");
                assert_eq!(
                    value["tools"],
                    json!([{"type":"file_search", "file_search_store_names":["fileSearchStores/test"]}])
                );
                json!({"steps":[{"type":"model_output", "content":[{"type":"text", "text":"A grounded answer.", "annotations":[{"type":"file_citation", "file_name":"guide.md", "source":"Approved guide", "page_number":2}]}]}]})
            }
            _ => panic!("Unexpected mock request: {path}"),
        };
        Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(output.to_string()))
            .unwrap()
    }
    async fn mock_api(
        root: &Path,
        foreign_upload: bool,
    ) -> (Gemini, Mock, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Mock {
            origin: origin.clone(),
            seen: Default::default(),
            foreign_upload,
        };
        let app = Router::new()
            .route("/{*path}", any(handler))
            .with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let api = Gemini::with_base(
            "test-secret",
            root,
            Url::parse(&format!("{origin}/")).unwrap(),
        )
        .unwrap();
        (api, state, server)
    }

    #[tokio::test]
    async fn real_http_shapes_for_create_pagination_upload_and_poll() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("guide.md"), "# Approved guide\n").unwrap();
        let (api, state, server) = mock_api(root.path(), false).await;
        assert_eq!(
            api.create_store("My new store").await.unwrap()["name"],
            "fileSearchStores/test"
        );
        let page = api.list_stores(2, Some("next")).await.unwrap();
        assert_eq!(page["nextPageToken"], "page-2");
        let upload = api
            .upload_file("fileSearchStores/test", "guide.md")
            .await
            .unwrap();
        assert_eq!(upload["done"], false);
        assert_eq!(
            api.get_operation(upload["name"].as_str().unwrap())
                .await
                .unwrap()["done"],
            true
        );
        assert_eq!(state.seen.lock().unwrap().len(), 5);
        server.abort();
    }

    #[tokio::test]
    async fn directory_uploads_eligible_files_in_order_and_keeps_pending_operation() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        for folder in ["z", "a with spaces"] {
            let directory = root.path().join(folder);
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(directory.join("guide.md"), "# Approved guide\n").unwrap();
        }
        std::fs::write(root.path().join("empty.md"), "").unwrap();
        std::fs::write(root.path().join("unsupported.csv"), "data").unwrap();
        std::fs::File::create(root.path().join("large.pdf"))
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path().join("z/guide.md"), root.path().join("copy.md"))
                .unwrap();
            std::os::unix::fs::symlink(outside.path(), root.path().join("external")).unwrap();
        }
        let (api, state, server) = mock_api(root.path(), false).await;
        let mut progress = Vec::new();
        let report = api
            .upload_directory("fileSearchStores/test", ".", None, |index, total, _| {
                progress.push((index, total));
            })
            .await
            .unwrap()
            .unwrap();
        assert!(report.error.is_none());
        assert_eq!(progress, vec![(1, 2), (2, 2)]);
        assert_eq!(
            Path::new(&report.files[0].path),
            Path::new("a with spaces/guide.md")
        );
        assert_eq!(Path::new(&report.files[1].path), Path::new("z/guide.md"));
        assert_eq!(state.seen.lock().unwrap().len(), 4);

        let report = api
            .upload_directory("fileSearchStores/test", "z", None, |_, _, _| {})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(Path::new(&report.files[0].path), Path::new("z/guide.md"));

        let report = api
            .upload_directory("fileSearchStores/test", ".", Some(0), |_, _, _| {})
            .await
            .unwrap()
            .unwrap();
        assert!(report.timed_out);
        assert_eq!(report.total, 2);
        assert_eq!(report.files.len(), 1);
        assert_eq!(
            report.files[0].operation.as_ref().unwrap()["name"],
            "fileSearchStores/test/upload/operations/upload-1"
        );
        assert!(
            api.upload_directory(
                "fileSearchStores/test",
                outside.path().to_str().unwrap(),
                None,
                |_, _, _| {}
            )
            .await
            .is_err()
        );
        assert_eq!(state.seen.lock().unwrap().len(), 8);
        server.abort();
    }

    #[tokio::test]
    async fn documents_deletion_and_cited_queries_use_documented_shapes() {
        let root = TempDir::new().unwrap();
        let (api, state, server) = mock_api(root.path(), false).await;
        let documents = api
            .list_documents("fileSearchStores/test", Some("next"))
            .await
            .unwrap();
        assert_eq!(documents["documents"][0]["state"], "STATE_ACTIVE");
        assert_eq!(documents["nextPageToken"], "page-2");
        let answer = api
            .ask(
                "fileSearchStores/test",
                "What does the guide say?",
                "test-model",
            )
            .await
            .unwrap();
        assert_eq!(answer.text, "A grounded answer.");
        assert_eq!(
            answer.citations,
            vec![Citation {
                file_name: "guide.md".into(),
                source: "Approved guide".into(),
                page_number: Some(2)
            }]
        );
        api.delete_document("fileSearchStores/test/documents/guide")
            .await
            .unwrap();
        api.delete_store("fileSearchStores/test").await.unwrap();
        assert_eq!(state.seen.lock().unwrap().len(), 4);
        server.abort();
    }

    #[tokio::test]
    async fn invalid_documents_and_queries_make_no_network_requests() {
        let root = TempDir::new().unwrap();
        let (api, state, server) = mock_api(root.path(), false).await;
        for name in [
            "https://evil.example",
            "fileSearchStores/test/documents/../escape",
            "fileSearchStores/test/documents/a?force=true",
        ] {
            assert!(api.delete_document(name).await.is_err());
        }
        assert!(api.ask("fileSearchStores/test", " ", "test").await.is_err());
        assert!(
            api.ask("fileSearchStores/test", &"a".repeat(32_001), "test")
                .await
                .is_err()
        );
        assert!(
            api.ask("fileSearchStores/test", "question", "")
                .await
                .is_err()
        );
        assert!(
            api.list_documents("fileSearchStores/test", Some(&"a".repeat(4097)))
                .await
                .is_err()
        );
        assert!(state.seen.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn bounded_wait_preserves_pending_and_failed_operations_without_writes() {
        let root = TempDir::new().unwrap();
        let (api, state, server) = mock_api(root.path(), false).await;
        let pending =
            json!({"name":"fileSearchStores/test/upload/operations/upload-1", "done":false});
        assert_eq!(
            api.wait_for_operation(pending.clone(), 0).await.unwrap(),
            pending
        );
        let failed = json!({"name":"fileSearchStores/test/upload/operations/upload-1", "done":true, "error":{"code":3}});
        assert_eq!(
            api.wait_for_operation(failed.clone(), 1).await.unwrap(),
            failed
        );
        assert!(state.seen.lock().unwrap().is_empty());
        server.abort();
    }

    #[test]
    fn answers_ignore_internal_steps_and_deduplicate_citations() {
        let citation = json!({"type":"file_citation", "file_name":"guide.md", "source":"excerpt"});
        let answer = parse_answer(&json!({"steps":[
            {"type":"thinking", "content":[{"type":"text", "text":"internal"}]},
            {"type":"model_output", "content":[
                {"type":"text", "text":"Part one", "annotations":[citation.clone(), citation]},
                {"type":"text", "text":"Part two"}
            ]}
        ]}))
        .unwrap();
        assert_eq!(answer.text, "Part one\n\nPart two");
        assert_eq!(answer.citations.len(), 1);
        assert!(parse_answer(&json!({"steps":[]})).is_err());
        assert!(
            parse_answer(&json!({"error":{"message":"provider private text"}}))
                .unwrap_err()
                .to_string()
                .find("private")
                .is_none()
        );
    }

    #[tokio::test]
    async fn does_not_echo_provider_errors_or_follow_foreign_upload_urls() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("guide.md"), "# Approved guide\n").unwrap();
        let (api, state, server) = mock_api(root.path(), true).await;
        let error = api
            .list_stores(2, Some("fail"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("403"));
        assert!(!error.contains("test-secret"));
        assert!(!error.contains("private provider content"));
        assert!(
            api.upload_file("fileSearchStores/test", "guide.md")
                .await
                .unwrap_err()
                .to_string()
                .contains("origin")
        );
        assert_eq!(state.seen.lock().unwrap().len(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn rejects_local_file_escape_and_invalid_uploads_before_network() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let api = Gemini::new("test-secret", root.path()).unwrap();
        let secret = outside.path().join("secret.md");
        std::fs::write(&secret, "private").unwrap();
        assert!(
            api.upload_file("fileSearchStores/test", secret.to_str().unwrap())
                .await
                .is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&secret, root.path().join("link.md")).unwrap();
            assert!(
                api.upload_file("fileSearchStores/test", "link.md")
                    .await
                    .is_err()
            );
        }
        std::fs::write(root.path().join("empty.md"), "").unwrap();
        std::fs::write(root.path().join("config.key"), "private").unwrap();
        let big = std::fs::File::create(root.path().join("big.md")).unwrap();
        big.set_len(MAX_FILE_BYTES + 1).unwrap();
        for name in ["empty.md", "config.key", "big.md", "missing.md"] {
            assert!(
                api.upload_file("fileSearchStores/test", name)
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn rejects_resource_path_injection_and_invalid_config() {
        for name in [
            "fileSearchStores/test/../other",
            "https://evil.example",
            "fileSearchStores/test?key=secret",
            "fileSearchStores/",
            "fileSearchStores/UPPER",
        ] {
            assert!(validate_store(name).is_err());
            assert!(validate_operation(name).is_err());
        }
        assert!(validate_operation("fileSearchStores/test/upload/operations/upload-1").is_ok());
        assert!(validate_operation("fileSearchStores/test/operations/op_2").is_ok());
        assert!(validate_operation("fileSearchStores/test/upload/operations/../escape").is_err());
        let root = TempDir::new().unwrap();
        assert!(Gemini::new("", root.path()).is_err());
        assert!(Gemini::new("line\nbreak", root.path()).is_err());
        assert!(Gemini::new("test-secret", &root.path().join("missing")).is_err());
    }
}
