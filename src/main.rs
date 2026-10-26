use gemini_file_search_mcp::api::{self, Gemini};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Clone)]
struct FileSearch {
    api: Gemini,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateStore {
    /// The store display name. Use 1 to 512 characters.
    display_name: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListStores {
    /// The number of stores per page (1 to 20). The default is 10.
    page_size: Option<u8>,
    /// Use nextPageToken from the last response. Do not change page_size.
    page_token: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UploadFile {
    /// Use the fileSearchStores/<id> resource name from create or list.
    store_name: String,
    /// The .md, .txt, or .pdf path in GEMINI_UPLOAD_ROOT. The maximum size is 10 MiB.
    path: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetOperation {
    /// Use the operation name from gemini_upload_file.
    name: String,
}

fn result(value: Result<Value, api::Error>) -> CallToolResult {
    match value {
        Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }
}

#[tool_router]
impl FileSearch {
    fn new(api: Gemini) -> Self {
        Self { api }
    }

    #[tool(
        description = "Create an empty Gemini File Search store at Google. Each call creates a new store. Examine the store list before you send a request again.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn gemini_create_store(
        &self,
        Parameters(args): Parameters<CreateStore>,
    ) -> CallToolResult {
        result(self.api.create_store(&args.display_name).await)
    }

    #[tool(
        description = "List the stores available to the API key. The response contains one page. Use nextPageToken to read more pages.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn gemini_list_stores(&self, Parameters(args): Parameters<ListStores>) -> CallToolResult {
        result(
            self.api
                .list_stores(args.page_size.unwrap_or(10), args.page_token.as_deref())
                .await,
        )
    }

    #[tool(
        description = "Upload and index one approved local file in the specified store. Use gemini_get_operation to read the result. A repeated upload can create one more copy and cause API charges.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn gemini_upload_file(&self, Parameters(args): Parameters<UploadFile>) -> CallToolResult {
        result(self.api.upload_file(&args.store_name, &args.path).await)
    }

    #[tool(
        description = "Read the upload operation status. Wait at least five seconds between requests. When done=true, examine error and response to find the result.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn gemini_get_operation(
        &self,
        Parameters(args): Parameters<GetOperation>,
    ) -> CallToolResult {
        result(self.api.get_operation(&args.name).await)
    }
}

#[tool_handler]
impl ServerHandler for FileSearch {}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        // Use stdout only for MCP messages.
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let key = std::env::var("GEMINI_FILE_SEARCH_API_KEY").map_err(|_| {
        "Set GEMINI_FILE_SEARCH_API_KEY. The server does not read other API key variables."
            .to_owned()
    })?;
    let root = std::env::var("GEMINI_UPLOAD_ROOT")
        .map_err(|_| "Set GEMINI_UPLOAD_ROOT to the approved document directory.".to_owned())?;
    let api = Gemini::new(&key, Path::new(&root)).map_err(|error| error.to_string())?;
    let server = FileSearch::new(api)
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await
        .map_err(|_| "The app cannot start the MCP server.".to_owned())?;
    server
        .waiting()
        .await
        .map_err(|_| "The MCP transport returned an error.".to_owned())?;
    Ok(())
}
