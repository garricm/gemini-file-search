use gemini_file_search_mcp::api::{Answer, Citation, DirectoryUpload, Gemini};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone)]
pub enum Backend {
    Live(Gemini),
    Demo(Arc<Mutex<DemoData>>),
}

pub struct DemoData {
    stores: Vec<Value>,
    documents: HashMap<String, Vec<Value>>,
    next_id: u64,
}

#[derive(Clone, Debug)]
pub enum Action {
    Stores(Option<String>),
    Documents {
        store: String,
        page: Option<String>,
    },
    Create(String),
    Upload {
        store: String,
        path: String,
    },
    Ask {
        store: String,
        question: String,
        model: String,
    },
    DeleteDocument(String),
    DeleteStore(String),
}

pub enum Update {
    Stores {
        value: Value,
        append: bool,
    },
    Documents {
        value: Value,
        append: bool,
    },
    Created(Value),
    UploadStarted(Value),
    Uploaded(Value),
    DirectoryUploadProgress {
        index: usize,
        total: usize,
        operation: Option<String>,
    },
    DirectoryUploaded {
        report: DirectoryUpload,
        documents: Result<Option<Value>, String>,
    },
    Answer {
        question: String,
        answer: Answer,
    },
    DeletedDocument(String),
    DeletedStore(String),
}

pub type Message = Result<Update, String>;

impl Backend {
    pub fn demo() -> Self {
        let stores = vec![
            json!({"name":"fileSearchStores/demo-documents", "displayName":"Documents", "activeDocumentsCount":"3"}),
            json!({"name":"fileSearchStores/demo-reference", "displayName":"Reference", "activeDocumentsCount":"2"}),
            json!({"name":"fileSearchStores/demo-notes", "displayName":"Notes", "activeDocumentsCount":"1"}),
        ];
        let documents = HashMap::from([
            (
                "fileSearchStores/demo-documents".into(),
                vec![
                    demo_document("demo-documents", "guide", "guide.md"),
                    demo_document("demo-documents", "reference", "reference.pdf"),
                    demo_document("demo-documents", "notes", "notes.txt"),
                ],
            ),
            (
                "fileSearchStores/demo-reference".into(),
                vec![
                    demo_document("demo-reference", "reference", "reference.md"),
                    demo_document("demo-reference", "guide", "guide.pdf"),
                ],
            ),
            (
                "fileSearchStores/demo-notes".into(),
                vec![demo_document("demo-notes", "notes", "notes.md")],
            ),
        ]);
        Self::Demo(Arc::new(Mutex::new(DemoData {
            stores,
            documents,
            next_id: 1,
        })))
    }

    pub fn is_demo(&self) -> bool {
        matches!(self, Self::Demo(_))
    }

    pub fn start(&self, action: Action, sender: UnboundedSender<Message>) {
        let backend = self.clone();
        tokio::spawn(async move {
            let result = match backend {
                Self::Live(api) => execute_live(api, action, &sender).await,
                Self::Demo(data) => {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    execute_demo(&data, action)
                }
            };
            let _ = sender.send(result);
        });
    }
}

async fn execute_live(api: Gemini, action: Action, sender: &UnboundedSender<Message>) -> Message {
    let outcome = match action {
        Action::Stores(page) => Update::Stores {
            value: api
                .list_stores(20, page.as_deref())
                .await
                .map_err(|e| e.to_string())?,
            append: page.is_some(),
        },
        Action::Documents { store, page } => Update::Documents {
            value: api
                .list_documents(&store, page.as_deref())
                .await
                .map_err(|e| e.to_string())?,
            append: page.is_some(),
        },
        Action::Create(name) => {
            Update::Created(api.create_store(&name).await.map_err(|e| e.to_string())?)
        }
        Action::Upload { store, path } => {
            if let Some(report) = api
                .upload_directory(&store, &path, Some(300), |index, total, operation| {
                    let _ = sender.send(Ok(Update::DirectoryUploadProgress {
                        index,
                        total,
                        operation: operation.map(str::to_owned),
                    }));
                })
                .await
                .map_err(|e| e.to_string())?
            {
                let indexed = report.files.iter().any(|file| {
                    file.operation.as_ref().is_some_and(|operation| {
                        operation["done"] == true && operation["error"].is_null()
                    })
                });
                let documents = if indexed {
                    api.list_documents(&store, None)
                        .await
                        .map(Some)
                        .map_err(|e| e.to_string())
                } else {
                    Ok(None)
                };
                return Ok(Update::DirectoryUploaded { report, documents });
            }
            let operation = api
                .upload_file(&store, &path)
                .await
                .map_err(|e| e.to_string())?;
            let _ = sender.send(Ok(Update::UploadStarted(operation.clone())));
            Update::Uploaded(
                api.wait_for_operation(operation, 300)
                    .await
                    .map_err(|e| e.to_string())?,
            )
        }
        Action::Ask {
            store,
            question,
            model,
        } => Update::Answer {
            answer: api
                .ask(&store, &question, &model)
                .await
                .map_err(|e| e.to_string())?,
            question,
        },
        Action::DeleteDocument(name) => {
            api.delete_document(&name)
                .await
                .map_err(|e| e.to_string())?;
            Update::DeletedDocument(name)
        }
        Action::DeleteStore(name) => {
            api.delete_store(&name).await.map_err(|e| e.to_string())?;
            Update::DeletedStore(name)
        }
    };
    Ok(outcome)
}

fn execute_demo(data: &Arc<Mutex<DemoData>>, action: Action) -> Message {
    let mut data = data
        .lock()
        .map_err(|_| "The app cannot open the sample data.")?;
    Ok(match action {
        Action::Stores(page) => Update::Stores {
            value: json!({"fileSearchStores":data.stores}),
            append: page.is_some(),
        },
        Action::Documents { store, page } => Update::Documents {
            value: json!({"documents": data.documents.get(&store).cloned().unwrap_or_default()}),
            append: page.is_some(),
        },
        Action::Create(name) => {
            if name.trim().is_empty() || name.chars().count() > 512 {
                return Err("The store name must contain 1 to 512 characters.".into());
            }
            let store = json!({"name":format!("fileSearchStores/demo-{}", data.next_id), "displayName":name, "activeDocumentsCount":"0"});
            data.next_id += 1;
            data.stores.push(store.clone());
            Update::Created(store)
        }
        Action::Upload { store, path } => {
            let file = std::path::Path::new(&path);
            if !matches!(
                file.extension().and_then(|ext| ext.to_str()),
                Some("md" | "txt" | "pdf")
            ) {
                return Err(
                    "Select a .md, .txt, or .pdf filename. Demo mode does not read local files."
                        .into(),
                );
            }
            let name = format!("{store}/documents/demo-{}", data.next_id);
            data.next_id += 1;
            data.documents.entry(store.clone()).or_default().push(json!({
                "name":name, "displayName": file.file_name().and_then(|v| v.to_str()).unwrap_or("demo.md"), "state":"STATE_ACTIVE"
            }));
            Update::Uploaded(
                json!({"name":format!("{store}/upload/operations/demo"), "done":true, "response":{}}),
            )
        }
        Action::Ask {
            store, question, ..
        } => {
            let documents = data.documents.get(&store).cloned().unwrap_or_default();
            let citations = documents
                .iter()
                .take(2)
                .map(|document| Citation {
                    file_name: document["displayName"].as_str().unwrap_or("demo.md").into(),
                    source:
                        "This is a sample source citation. Demo mode does not read local files."
                            .into(),
                    page_number: None,
                })
                .collect();
            Update::Answer { question, answer: Answer {
                text: "This is a sample answer.\n\nWith API access, Gemini uses the documents in the open store to answer your question. The response includes the available source citations.\n\nPress Tab to select a panel. Press / to filter the document names. Press u to add a sample document. Demo mode does not read files or call Google.".into(),
                citations,
            }}
        }
        Action::DeleteDocument(name) => {
            for documents in data.documents.values_mut() {
                documents.retain(|document| document["name"] != name);
            }
            Update::DeletedDocument(name)
        }
        Action::DeleteStore(name) => {
            data.stores.retain(|store| store["name"] != name);
            data.documents.remove(&name);
            Update::DeletedStore(name)
        }
    })
}

fn demo_document(store: &str, id: &str, label: &str) -> Value {
    json!({"name": format!("fileSearchStores/{store}/documents/{id}"), "displayName":label, "state":"STATE_ACTIVE"})
}
