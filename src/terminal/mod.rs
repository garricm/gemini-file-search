mod backend;
mod render;
#[cfg(test)]
mod tests;

pub use backend::Backend;
use backend::{Action, Message, Update};
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
};
use gemini_file_search_mcp::api::Answer;
use serde_json::Value;
use std::{io, time::Duration};
use tokio::sync::mpsc::{self, UnboundedSender};

/// Remove terminal control characters from display text.
pub fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            (!c.is_control() || matches!(c, '\n' | '\t'))
                && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

#[derive(Clone, Debug)]
struct Item {
    name: String,
    label: String,
    state: String,
}

impl Item {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            name: value.get("name")?.as_str()?.to_owned(),
            label: clean(
                value
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("Resource"),
            ),
            state: clean(
                value
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Focus {
    Stores,
    Documents,
    Answer,
}

#[derive(Clone, Debug)]
enum Dialog {
    Create,
    Upload,
    Ask,
    Search,
    DeleteDocument(Item),
    DeleteStore(Item),
    Quit,
    Help,
}

#[derive(Default)]
struct Input {
    text: String,
    cursor: usize,
}

impl Input {
    fn insert(&mut self, value: &str) {
        let value = clean(value).replace(['\n', '\t'], " ");
        for character in value.chars() {
            if self.text.len() + character.len_utf8() > 32_000 {
                break;
            }
            self.text.insert(self.cursor, character);
            self.cursor += character.len_utf8();
        }
    }

    fn key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                *self = Self::default()
            }
            KeyCode::Char(value)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&value.to_string())
            }
            KeyCode::Left => {
                self.cursor = self.text[..self.cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            }
            KeyCode::Right => {
                if let Some(value) = self.text[self.cursor..].chars().next() {
                    self.cursor += value.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                let previous = self.text[..self.cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.text.drain(previous..self.cursor);
                self.cursor = previous;
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                let length = self.text[self.cursor..].chars().next().unwrap().len_utf8();
                self.text.drain(self.cursor..self.cursor + length);
            }
            _ => {}
        }
    }
}

struct App {
    stores: Vec<Item>,
    documents: Vec<Item>,
    store_cursor: usize,
    document_cursor: usize,
    active: Option<Item>,
    stores_next: Option<String>,
    documents_next: Option<String>,
    focus: Focus,
    dialog: Option<Dialog>,
    input: Input,
    filter: String,
    answer: Option<(String, Answer)>,
    answer_scroll: u16,
    status: String,
    error: bool,
    busy: bool,
    operation: Option<String>,
    model: String,
    demo: bool,
    tick: usize,
    quit: bool,
}

impl App {
    fn new(model: String, demo: bool) -> Self {
        Self {
            stores: Vec::new(),
            documents: Vec::new(),
            store_cursor: 0,
            document_cursor: 0,
            active: None,
            stores_next: None,
            documents_next: None,
            focus: Focus::Stores,
            dialog: None,
            input: Input::default(),
            filter: String::new(),
            answer: None,
            answer_scroll: 0,
            status: "Wait for the interface…".into(),
            error: false,
            busy: false,
            operation: None,
            model,
            demo,
            tick: 0,
            quit: false,
        }
    }

    fn visible_documents(&self) -> Vec<&Item> {
        let filter = self.filter.to_lowercase();
        self.documents
            .iter()
            .filter(|item| item.label.to_lowercase().contains(&filter))
            .collect()
    }

    fn open_dialog(&mut self, dialog: Dialog) {
        self.input = Input::default();
        if matches!(dialog, Dialog::Search) {
            self.input.insert(&self.filter);
        }
        self.dialog = Some(dialog);
    }

    fn request(&mut self, action: Action, backend: &Backend, sender: &UnboundedSender<Message>) {
        if self.busy {
            return;
        }
        self.status = match &action {
            Action::Stores(_) => "Wait for the store list…".into(),
            Action::Documents { .. } => "Wait for the document list…".into(),
            Action::Create(_) => "Wait for the new store…".into(),
            Action::Upload { .. } => "Wait for the file upload…".into(),
            Action::Ask { .. } => "Wait for the answer…".into(),
            Action::DeleteDocument(_) | Action::DeleteStore(_) => "Wait for the deletion…".into(),
        };
        if matches!(action, Action::Upload { .. }) {
            self.operation = None;
        }
        self.error = false;
        self.busy = true;
        backend.start(action, sender.clone());
    }

    fn activate(&mut self, store: Item, backend: &Backend, sender: &UnboundedSender<Message>) {
        if self.busy {
            return;
        }
        self.documents.clear();
        self.document_cursor = 0;
        self.documents_next = None;
        self.filter.clear();
        self.answer = None;
        self.answer_scroll = 0;
        let name = store.name.clone();
        self.active = Some(store);
        self.request(
            Action::Documents {
                store: name,
                page: None,
            },
            backend,
            sender,
        );
    }

    fn receive(&mut self, message: Message, backend: &Backend, sender: &UnboundedSender<Message>) {
        self.busy = false;
        let update = match message {
            Ok(update) => update,
            Err(error) => {
                self.status = clean(&error);
                self.error = true;
                return;
            }
        };
        self.error = false;
        match update {
            Update::Stores { value, append } => {
                merge_items(&mut self.stores, &value, "fileSearchStores", append);
                self.stores_next = next_page(&value);
                self.store_cursor = self.store_cursor.min(self.stores.len().saturating_sub(1));
                self.status = format!(
                    "{} stores available · Press Enter to open a store",
                    self.stores.len()
                );
                if let Some(active) = &self.active
                    && !self.stores.iter().any(|item| item.name == active.name)
                {
                    self.active = None;
                    self.documents.clear();
                    self.answer = None;
                }
                if self.active.is_none()
                    && let Some(store) = self.stores.first().cloned()
                {
                    self.activate(store, backend, sender);
                }
            }
            Update::Documents { value, append } => {
                merge_items(&mut self.documents, &value, "documents", append);
                self.documents_next = next_page(&value);
                self.document_cursor = self
                    .document_cursor
                    .min(self.visible_documents().len().saturating_sub(1));
                self.status = format!(
                    "{} documents available · Press a to enter a question",
                    self.documents.len()
                );
            }
            Update::Created(value) => {
                if let Some(store) = Item::from_value(&value) {
                    self.stores.push(store.clone());
                    self.store_cursor = self.stores.len() - 1;
                    self.activate(store, backend, sender);
                } else {
                    self.status =
                        "The response does not include the store ID. Reload the list before you send the request again.".into();
                    self.error = true;
                }
            }
            Update::UploadStarted(operation) => {
                self.operation = operation["name"].as_str().map(str::to_owned);
                self.status = "Google received the file. Wait for indexing…".into();
                self.busy = true;
            }
            Update::Uploaded(operation) => {
                self.operation = operation["name"].as_str().map(str::to_owned);
                if operation.get("error").is_some_and(|error| !error.is_null()) {
                    self.status =
                        "Indexing did not complete. Examine the operation before you upload the file again.".into();
                    self.error = true;
                } else if operation["done"] != true {
                    self.status =
                        "Indexing is not complete. Continue with gfs operation <ID> --wait.".into();
                } else {
                    self.status = "Google indexed the document.".into();
                    if let Some(store) = &self.active {
                        self.request(
                            Action::Documents {
                                store: store.name.clone(),
                                page: None,
                            },
                            backend,
                            sender,
                        );
                    }
                }
            }
            Update::Answer { question, answer } => {
                self.status = format!("Answer available · {} sources", answer.citations.len());
                self.answer = Some((question, answer));
                self.answer_scroll = 0;
                self.focus = Focus::Answer;
            }
            Update::DeletedDocument(name) => {
                self.documents.retain(|document| document.name != name);
                self.document_cursor = self
                    .document_cursor
                    .min(self.visible_documents().len().saturating_sub(1));
                self.answer = None;
                self.status = "The app deleted the document.".into();
            }
            Update::DeletedStore(name) => {
                self.stores.retain(|store| store.name != name);
                self.store_cursor = self.store_cursor.min(self.stores.len().saturating_sub(1));
                if self.active.as_ref().is_some_and(|store| store.name == name) {
                    self.active = None;
                    self.documents.clear();
                    self.answer = None;
                    if let Some(store) = self.stores.get(self.store_cursor).cloned() {
                        self.activate(store, backend, sender);
                    }
                }
                if !self.busy {
                    self.status = "The app deleted the store and its documents.".into();
                }
            }
        }
    }

    fn submit(&mut self, backend: &Backend, sender: &UnboundedSender<Message>) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let value = self.input.text.trim().to_owned();
        if matches!(dialog, Dialog::Help) {
            self.dialog = None;
            return;
        }
        if matches!(dialog, Dialog::Search) {
            self.filter = value;
            self.document_cursor = 0;
            self.dialog = None;
            return;
        }
        if value.is_empty() {
            return;
        }
        let action = match dialog {
            Dialog::Create => Some(Action::Create(value)),
            Dialog::Upload => self.active.as_ref().map(|store| Action::Upload {
                store: store.name.clone(),
                path: value,
            }),
            Dialog::Ask => self.active.as_ref().map(|store| Action::Ask {
                store: store.name.clone(),
                question: value,
                model: self.model.clone(),
            }),
            Dialog::DeleteDocument(item) if value == "delete" => {
                Some(Action::DeleteDocument(item.name))
            }
            Dialog::DeleteStore(item) if value == "delete" => Some(Action::DeleteStore(item.name)),
            Dialog::Quit if value == "quit" => {
                self.quit = true;
                None
            }
            _ => return,
        };
        self.dialog = None;
        if let Some(action) = action {
            self.request(action, backend, sender);
        }
    }

    fn key(&mut self, key: KeyEvent, backend: &Backend, sender: &UnboundedSender<Message>) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.busy {
                self.open_dialog(Dialog::Quit);
            } else {
                self.quit = true;
            }
            return;
        }
        if self.dialog.is_some() {
            match key.code {
                KeyCode::Esc => self.dialog = None,
                KeyCode::Enter => self.submit(backend, sender),
                _ => self.input.key(key),
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => {
                if self.busy {
                    self.open_dialog(Dialog::Quit);
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('?') => self.open_dialog(Dialog::Help),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match (self.focus, key.code) {
                    (Focus::Stores, KeyCode::BackTab) | (Focus::Documents, KeyCode::Tab) => {
                        Focus::Answer
                    }
                    (Focus::Documents, KeyCode::BackTab) | (Focus::Answer, KeyCode::Tab) => {
                        Focus::Stores
                    }
                    _ => Focus::Documents,
                }
            }
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Stores => self.store_cursor = self.store_cursor.saturating_sub(1),
                Focus::Documents => self.document_cursor = self.document_cursor.saturating_sub(1),
                Focus::Answer => self.answer_scroll = self.answer_scroll.saturating_sub(1),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Stores => {
                    self.store_cursor =
                        (self.store_cursor + 1).min(self.stores.len().saturating_sub(1))
                }
                Focus::Documents => {
                    self.document_cursor = (self.document_cursor + 1)
                        .min(self.visible_documents().len().saturating_sub(1))
                }
                Focus::Answer => self.answer_scroll = self.answer_scroll.saturating_add(1),
            },
            KeyCode::PageUp => self.answer_scroll = self.answer_scroll.saturating_sub(8),
            KeyCode::PageDown => self.answer_scroll = self.answer_scroll.saturating_add(8),
            KeyCode::Home if self.focus == Focus::Answer => self.answer_scroll = 0,
            KeyCode::Enter if self.focus == Focus::Stores && !self.busy => {
                if let Some(store) = self.stores.get(self.store_cursor).cloned() {
                    self.activate(store, backend, sender);
                    self.focus = Focus::Documents;
                }
            }
            KeyCode::Char('/') => self.open_dialog(Dialog::Search),
            KeyCode::Esc => {
                self.filter.clear();
                self.document_cursor = 0;
            }
            KeyCode::Char('n') if !self.busy => self.open_dialog(Dialog::Create),
            KeyCode::Char('u') if !self.busy && self.active.is_some() => {
                self.open_dialog(Dialog::Upload)
            }
            KeyCode::Char('a') if !self.busy && self.active.is_some() => {
                self.open_dialog(Dialog::Ask)
            }
            KeyCode::Char('d') if !self.busy => match self.focus {
                Focus::Documents => {
                    if let Some(item) = self
                        .visible_documents()
                        .get(self.document_cursor)
                        .cloned()
                        .cloned()
                    {
                        self.open_dialog(Dialog::DeleteDocument(item));
                    }
                }
                Focus::Stores => {
                    if let Some(item) = self.stores.get(self.store_cursor).cloned() {
                        self.open_dialog(Dialog::DeleteStore(item));
                    }
                }
                _ => {}
            },
            KeyCode::Char('r') if !self.busy => {
                if self.focus == Focus::Stores {
                    self.request(Action::Stores(None), backend, sender);
                } else if let Some(store) = &self.active {
                    self.request(
                        Action::Documents {
                            store: store.name.clone(),
                            page: None,
                        },
                        backend,
                        sender,
                    );
                }
            }
            KeyCode::Char('m') if !self.busy => {
                if self.focus == Focus::Stores
                    && let Some(page) = self.stores_next.clone()
                {
                    self.request(Action::Stores(Some(page)), backend, sender);
                } else if self.focus == Focus::Documents
                    && let (Some(store), Some(page)) = (&self.active, self.documents_next.clone())
                {
                    self.request(
                        Action::Documents {
                            store: store.name.clone(),
                            page: Some(page),
                        },
                        backend,
                        sender,
                    );
                }
            }
            _ => {}
        }
    }
}

fn next_page(value: &Value) -> Option<String> {
    value
        .get("nextPageToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

fn merge_items(items: &mut Vec<Item>, value: &Value, key: &str, append: bool) {
    if !append {
        items.clear();
    }
    if let Some(rows) = value.get(key).and_then(Value::as_array) {
        for row in rows {
            if let Some(item) = Item::from_value(row) {
                if let Some(existing) = items.iter_mut().find(|existing| existing.name == item.name)
                {
                    *existing = item;
                } else {
                    items.push(item);
                }
            }
        }
    }
}

struct RestoreTerminal;
impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        ratatui::restore();
    }
}

pub async fn run(backend: Backend, model: String) -> io::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let _restore = RestoreTerminal;
    execute!(io::stdout(), EnableBracketedPaste)?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut app = App::new(model, backend.is_demo());
    app.request(Action::Stores(None), &backend, &sender);
    while !app.quit {
        while let Ok(message) = receiver.try_recv() {
            app.receive(message, &backend, &sender);
        }
        terminal.draw(|frame| render::draw(frame, &mut app))?;
        // Process terminal input separately from HTTP requests.
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(key) => app.key(key, &backend, &sender),
                Event::Paste(value) if app.dialog.is_some() => app.input.insert(&value),
                _ => {}
            }
        }
        app.tick = app.tick.wrapping_add(1);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    Ok(())
}
