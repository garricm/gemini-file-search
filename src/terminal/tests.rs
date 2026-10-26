use super::*;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn store() -> Item {
    Item {
        name: "fileSearchStores/demo-documents".into(),
        label: "Documents".into(),
        state: String::new(),
    }
}

fn populated() -> App {
    let mut app = App::new("demo-model".into(), true);
    app.stores.push(store());
    app.active = Some(store());
    app.documents = vec![
        Item {
            name: "fileSearchStores/demo-documents/documents/a".into(),
            label: "guide.md".into(),
            state: "STATE_ACTIVE".into(),
        },
        Item {
            name: "fileSearchStores/demo-documents/documents/b".into(),
            label: "reference.pdf".into(),
            state: "STATE_PENDING".into(),
        },
    ];
    app.status = "The sample data is available.".into();
    app.answer = Some((
        "What does the guide specify?".into(),
        Answer {
            text: "Read the guide.\n\nThis is a sample answer from demo mode.".into(),
            citations: vec![gemini_file_search_mcp::api::Citation {
                file_name: "guide.md".into(),
                source: "The guide contains sample instructions.".into(),
                page_number: Some(1),
            }],
        },
    ));
    app
}

#[test]
fn unicode_input_edits_at_character_boundaries_and_sanitizes_pastes() {
    let mut input = Input::default();
    input.insert("hé🌿界");
    input.key(key(KeyCode::Left));
    input.key(key(KeyCode::Backspace));
    input.insert("!");
    assert_eq!(input.text, "hé!界");
    input.key(key(KeyCode::Home));
    input.key(key(KeyCode::Delete));
    assert_eq!(input.text, "é!界");
    input.key(key(KeyCode::End));
    input.insert("\x1b[31m\r\ntext\u{202e}");
    assert!(!input.text.contains('\x1b'));
    assert!(!input.text.contains('\u{202e}'));
    assert!(!input.text.contains('\n'));
    input.insert(&"界".repeat(32_000));
    assert!(input.text.len() <= 32_000);
}

#[test]
fn pagination_merges_without_duplicates_and_refresh_replaces() {
    let mut items = vec![];
    let first = json!({"documents":[{"name":"one", "displayName":"First"}]});
    merge_items(&mut items, &first, "documents", false);
    merge_items(
        &mut items,
        &json!({"documents":[{"name":"one","displayName":"Updated"},{"name":"two"}]}),
        "documents",
        true,
    );
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].label, "Updated");
    merge_items(&mut items, &first, "documents", false);
    assert_eq!(items.len(), 1);
}

#[test]
fn filtering_and_busy_navigation_do_not_switch_the_request_store() {
    let mut app = populated();
    let backend = Backend::demo();
    let (sender, _receiver) = mpsc::unbounded_channel();
    app.open_dialog(Dialog::Search);
    app.input.insert("REFERENCE");
    app.submit(&backend, &sender);
    assert_eq!(app.visible_documents().len(), 1);
    assert_eq!(app.visible_documents()[0].label, "reference.pdf");
    app.key(key(KeyCode::Esc), &backend, &sender);
    assert_eq!(app.visible_documents().len(), 2);
    app.busy = true;
    app.stores.push(Item {
        name: "fileSearchStores/other".into(),
        ..store()
    });
    app.store_cursor = 1;
    app.key(key(KeyCode::Enter), &backend, &sender);
    assert_eq!(app.active.unwrap().name, store().name);
}

#[test]
fn destructive_dialog_needs_exact_confirmation_and_quit_warns_during_work() {
    let mut app = populated();
    let backend = Backend::demo();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    app.open_dialog(Dialog::DeleteStore(store()));
    app.input.insert("yes");
    app.submit(&backend, &sender);
    assert!(app.dialog.is_some());
    assert!(!app.busy);
    assert!(receiver.try_recv().is_err());
    app.key(key(KeyCode::Esc), &backend, &sender);
    app.busy = true;
    app.key(key(KeyCode::Char('q')), &backend, &sender);
    assert!(!app.quit);
    assert!(matches!(app.dialog, Some(Dialog::Quit)));
    app.input.insert("quit");
    app.submit(&backend, &sender);
    assert!(app.quit);
}

#[test]
fn failed_indexing_is_not_reported_as_success_and_keeps_operation_handle() {
    let mut app = populated();
    let backend = Backend::demo();
    let (sender, _) = mpsc::unbounded_channel();
    app.receive(Ok(Update::Uploaded(json!({"name":"fileSearchStores/test/upload/operations/fail", "done":true, "error":{"message":"private provider content"}}))), &backend, &sender);
    assert!(app.error);
    assert!(app.operation.unwrap().ends_with("/fail"));
    assert!(!app.status.contains("private provider content"));
}

#[tokio::test]
async fn demo_supports_create_upload_query_and_delete_without_credentials() {
    let backend = Backend::demo();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    backend.start(Action::Create("Test collection".into()), sender.clone());
    let created = match receiver.recv().await.unwrap().unwrap() {
        Update::Created(value) => value,
        _ => panic!("The response does not contain the created store."),
    };
    let name = created["name"].as_str().unwrap().to_owned();
    backend.start(
        Action::Upload {
            store: name.clone(),
            path: "/this/file/does/not/exist.md".into(),
        },
        sender.clone(),
    );
    assert!(matches!(
        receiver.recv().await.unwrap().unwrap(),
        Update::Uploaded(_)
    ));
    backend.start(
        Action::Documents {
            store: name.clone(),
            page: None,
        },
        sender.clone(),
    );
    let document = match receiver.recv().await.unwrap().unwrap() {
        Update::Documents { value, .. } => {
            assert_eq!(value["documents"].as_array().unwrap().len(), 1);
            value["documents"][0]["name"].as_str().unwrap().to_owned()
        }
        _ => panic!("The response does not contain documents."),
    };
    backend.start(
        Action::Ask {
            store: name.clone(),
            question: "Anything?".into(),
            model: "unused".into(),
        },
        sender.clone(),
    );
    match receiver.recv().await.unwrap().unwrap() {
        Update::Answer { answer, .. } => {
            assert!(answer.text.contains("sample answer"));
            assert_eq!(answer.citations.len(), 1);
        }
        _ => panic!("The response does not contain an answer."),
    }
    backend.start(Action::DeleteDocument(document), sender.clone());
    assert!(matches!(
        receiver.recv().await.unwrap().unwrap(),
        Update::DeletedDocument(_)
    ));
    backend.start(Action::DeleteStore(name), sender);
    assert!(matches!(
        receiver.recv().await.unwrap().unwrap(),
        Update::DeletedStore(_)
    ));
}

#[test]
fn renders_workspace_and_dialogs_across_terminal_sizes_without_panics() {
    for (width, height) in [(120, 40), (80, 24), (64, 20), (30, 10), (1, 1)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut app = populated();
        app.answer_scroll = u16::MAX;
        terminal
            .draw(|frame| render::draw(frame, &mut app))
            .unwrap();
        if width == 120
            && let Ok(path) = std::env::var("GFS_PREVIEW_PATH")
        {
            let cells = terminal.backend().buffer().content.iter().map(|cell| {
                fn rgb(color: ratatui::style::Color) -> [u8; 3] {
                    match color { ratatui::style::Color::Rgb(r,g,b) => [r,g,b], _ => [10,16,23] }
                }
                json!({"text":cell.symbol(), "fg":rgb(cell.fg), "bg":rgb(cell.bg), "bold":cell.modifier.contains(ratatui::style::Modifier::BOLD)})
            }).collect::<Vec<_>>();
            std::fs::write(
                path,
                serde_json::to_vec(&json!({"width":width,"height":height,"cells":cells})).unwrap(),
            )
            .unwrap();
        }
        if width >= 64 {
            let snapshot: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(snapshot.contains("GFS"));
            assert!(snapshot.contains("STORES"));
        }
        for dialog in [
            Dialog::Create,
            Dialog::Upload,
            Dialog::Ask,
            Dialog::Search,
            Dialog::Help,
            Dialog::DeleteStore(store()),
            Dialog::Quit,
        ] {
            app.open_dialog(dialog);
            app.input.insert(&"界🌿".repeat(100));
            terminal
                .draw(|frame| render::draw(frame, &mut app))
                .unwrap();
        }
    }
}
