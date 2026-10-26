use super::{App, Dialog, Focus, clean};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Row,
        Table, TableState, Wrap,
    },
};

const BG: Color = Color::Rgb(10, 16, 23);
const PANEL: Color = Color::Rgb(15, 24, 34);
const EDGE: Color = Color::Rgb(39, 57, 72);
const TEAL: Color = Color::Rgb(83, 220, 194);
const TEXT: Color = Color::Rgb(223, 233, 240);
const MUTED: Color = Color::Rgb(139, 160, 177);
const GOLD: Color = Color::Rgb(240, 190, 105);
const RED: Color = Color::Rgb(247, 137, 143);
const SELECTED: Color = Color::Rgb(24, 54, 61);

fn panel(title: impl Into<Line<'static>>, active: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if active { TEAL } else { EDGE }))
        .title_style(Style::default().fg(if active { TEAL } else { MUTED }))
        .style(Style::default().bg(PANEL).fg(TEXT))
        .title(title)
        .padding(Padding::horizontal(1))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    if area.width < 64 || area.height < 20 {
        let text = if matches!(app.dialog, Some(Dialog::Quit)) {
            format!(
                "A request is not complete.\nGoogle can still process the request.\nType quit and press Enter to close the app.\n\n> {}",
                app.input.text
            )
        } else {
            "GFS · File Search\n\nSet the terminal size to at least 64 columns and 20 rows.\nPress Esc to close a dialog.\nPress q to close the app.".into()
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(TEAL))
                .wrap(Wrap { trim: false })
                .block(panel(" Terminal too small ", true)),
            area,
        );
        return;
    }
    let area = area.inner(Margin::new(1, 0));
    let [header, main, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(10),
        Constraint::Length(4),
    ])
    .areas(area);
    header_view(frame, header, app);
    let sidebar_width = (main.width / 4).clamp(23, 34);
    let [sidebar, workspace] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(30)])
            .spacing(1)
            .areas(main);
    stores_view(frame, sidebar, app);
    let [summary, documents, answer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length((workspace.height / 3).clamp(5, 11)),
        Constraint::Min(3),
    ])
    .spacing(1)
    .areas(workspace);
    summary_view(frame, summary, app);
    documents_view(frame, documents, app);
    answer_view(frame, answer, app);
    footer_view(frame, footer, app);
    if let Some(dialog) = &app.dialog {
        dialog_view(frame, area, app, dialog);
    }
}

fn header_view(frame: &mut Frame, area: Rect, app: &App) {
    let badge = if app.demo {
        " OFFLINE DEMO "
    } else {
        " LIVE · GOOGLE "
    };
    let title = Line::from(vec![
        Span::styled(
            " ◈ GFS ",
            Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" /  FILE SEARCH", Style::default().fg(TEXT)),
        Span::raw("   "),
        Span::styled(
            badge,
            Style::default()
                .fg(if app.demo { GOLD } else { TEAL })
                .bg(PANEL),
        ),
    ]);
    let subtitle = if app.demo {
        "  Demo mode uses sample data. It does not read files or call Google."
    } else {
        "  Manage your documents. Enter questions and read source citations."
    };
    frame.render_widget(
        Paragraph::new(vec![
            title,
            Line::from(Span::styled(subtitle, Style::default().fg(MUTED))),
        ])
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(EDGE)),
        ),
        area,
    );
}

fn stores_view(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!(" 01  STORES · {} ", app.stores.len());
    let block = panel(title, app.focus == Focus::Stores).title_bottom(
        Line::from(if app.stores_next.is_some() {
            " n new  ·  m more "
        } else {
            " n new  ·  Enter open "
        })
        .style(Style::default().fg(MUTED)),
    );
    if app.stores.is_empty() {
        frame.render_widget(
            Paragraph::new("\nThe store list is empty.\n\nPress n to create\na store.")
                .style(Style::default().fg(MUTED))
                .block(block),
            area,
        );
        return;
    }
    let rows = app
        .stores
        .iter()
        .map(|store| {
            let active = app
                .active
                .as_ref()
                .is_some_and(|active| active.name == store.name);
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        if active { "● " } else { "○ " },
                        Style::default().fg(if active { TEAL } else { MUTED }),
                    ),
                    Span::raw(store.label.replace(['\n', '\t'], " ")),
                ]),
                Line::from(Span::styled(
                    if active {
                        "  Open store"
                    } else {
                        "  Enter: Open"
                    },
                    Style::default().fg(MUTED),
                )),
                Line::from(""),
            ])
        })
        .collect::<Vec<_>>();
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::default().bg(SELECTED).fg(TEXT))
        .highlight_symbol("▎");
    let mut state = ListState::default().with_selected(Some(app.store_cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

fn summary_view(frame: &mut Frame, area: Rect, app: &App) {
    let (name, id) = app
        .active
        .as_ref()
        .map(|item| (item.label.clone(), clean(&item.name)))
        .unwrap_or_else(|| {
            (
                "Select a store".into(),
                "Select a store and press Enter to open it.".into(),
            )
        });
    let content = vec![
        Line::from(Span::styled(
            name.replace(['\n', '\t'], " "),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(id, Style::default().fg(MUTED))),
    ];
    frame.render_widget(
        Paragraph::new(content).block(panel(" OPEN STORE ", false)),
        area,
    );
}

fn state_label(state: &str) -> (&str, Color) {
    match state {
        "STATE_ACTIVE" | "ACTIVE" => ("● Active", TEAL),
        "STATE_PENDING" | "PENDING" => ("◌ Indexing", GOLD),
        "STATE_FAILED" | "FAILED" => ("× Failed", RED),
        _ => ("· No status", MUTED),
    }
}

fn documents_view(frame: &mut Frame, area: Rect, app: &App) {
    let documents = app.visible_documents();
    let filter = if app.filter.is_empty() {
        String::new()
    } else {
        format!(" · /{}", app.filter)
    };
    let title = format!(" 02  DOCUMENTS · {}{} ", documents.len(), filter);
    let block = panel(title, app.focus == Focus::Documents).title_bottom(
        Line::from(if app.documents_next.is_some() {
            " u upload  ·  / filter  ·  m more "
        } else {
            " u upload  ·  / filter "
        })
        .style(Style::default().fg(MUTED)),
    );
    if documents.is_empty() {
        let text = if !app.filter.is_empty() {
            "No document names match. Press Esc to remove the filter."
        } else if app.active.is_some() {
            "The document list is empty. Press u to upload a file."
        } else {
            "Open a store to see its documents."
        };
        frame.render_widget(
            Paragraph::new(format!("\n{text}"))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(MUTED))
                .block(block),
            area,
        );
        return;
    }
    let rows = documents.iter().map(|document| {
        let (label, color) = state_label(&document.state);
        Row::new(vec![
            Text::from(document.label.replace(['\n', '\t'], " ")),
            Text::from(Span::styled(label, Style::default().fg(color))),
        ])
    });
    let table = Table::new(rows, [Constraint::Min(12), Constraint::Length(12)])
        .header(
            Row::new(["NAME", "STATUS"])
                .style(Style::default().fg(MUTED))
                .bottom_margin(1),
        )
        .block(block)
        .column_spacing(2)
        .row_highlight_style(Style::default().bg(SELECTED))
        .highlight_symbol("▎");
    let mut state = TableState::default().with_selected(Some(app.document_cursor));
    frame.render_stateful_widget(table, area, &mut state);
}

fn answer_view(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = panel(" 03  QUESTIONS ", app.focus == Focus::Answer)
        .title_bottom(Line::from(" a question  ·  ↑↓ scroll ").style(Style::default().fg(MUTED)));
    let mut lines = Vec::new();
    if let Some((question, answer)) = &app.answer {
        lines.push(Line::from(Span::styled(
            format!("You / {}", clean(question)),
            Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.extend(
            clean(&answer.text)
                .lines()
                .map(|line| Line::from(line.to_owned())),
        );
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("SOURCES · {}", answer.citations.len()),
            Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
        )));
        if answer.citations.is_empty() {
            lines.push(Line::from(Span::styled(
                "Gemini did not return citations. Compare the answer with your documents.",
                Style::default().fg(GOLD),
            )));
        }
        for (index, citation) in answer.citations.iter().enumerate() {
            let page = citation
                .page_number
                .map(|number| format!(" · page {number}"))
                .unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!("[{}] {}{}", index + 1, clean(&citation.file_name), page),
                Style::default().fg(GOLD),
            )));
            lines.extend(
                clean(&citation.source).lines().map(|line| {
                    Line::from(Span::styled(line.to_owned(), Style::default().fg(MUTED)))
                }),
            );
            lines.push(Line::from(""));
        }
    } else {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                "Enter a question about the open store.",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "The answer includes the available source citations.",
                Style::default().fg(MUTED),
            )),
            Line::from(Span::styled(
                "Press a to enter a question.",
                Style::default().fg(TEAL),
            )),
        ]);
    }
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let count = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize);
    app.answer_scroll = app.answer_scroll.min(count.min(u16::MAX as usize) as u16);
    frame.render_widget(paragraph.scroll((app.answer_scroll, 0)).block(block), area);
}

fn footer_view(frame: &mut Frame, area: Rect, app: &App) {
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let indicator = if app.busy {
        spinner[(app.tick / 2) % spinner.len()]
    } else if app.error {
        "×"
    } else {
        "●"
    };
    let color = if app.error { RED } else { TEAL };
    let recovery = app
        .operation
        .as_ref()
        .map(|name| format!("  Operation / {}", clean(name)))
        .unwrap_or_else(|| {
            format!(
                "  Model / {}{}",
                clean(&app.model),
                if app.demo { " · sample data" } else { "" }
            )
        });
    let lines = vec![
        Line::from(vec![
            Span::styled(format!(" {indicator} "), Style::default().fg(color)),
            Span::styled(
                clean(&app.status),
                Style::default().fg(if app.error { RED } else { TEXT }),
            ),
        ]),
        Line::from(Span::styled(recovery, Style::default().fg(MUTED))),
        Line::from(vec![
            Span::styled(" Tab", Style::default().fg(TEAL)),
            Span::raw(" panels   "),
            Span::styled("n", Style::default().fg(TEAL)),
            Span::raw(" new   "),
            Span::styled("u", Style::default().fg(TEAL)),
            Span::raw(" upload   "),
            Span::styled("a", Style::default().fg(TEAL)),
            Span::raw(" question   "),
            Span::styled("?", Style::default().fg(TEAL)),
            Span::raw(" help   "),
            Span::styled("q", Style::default().fg(TEAL)),
            Span::raw(" close"),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(EDGE)),
        ),
        area,
    );
}

fn dialog_view(frame: &mut Frame, area: Rect, app: &App, dialog: &Dialog) {
    let (title, description, danger) = match dialog {
        Dialog::Create => (" NEW STORE ", "Enter a store name. Each request creates a new store.".into(), false),
        Dialog::Upload => (" UPLOAD DOCUMENT ", if app.demo { "Enter a .md, .txt, or .pdf path. Demo mode uses sample data.".into() }
            else { "Enter a file path in GEMINI_UPLOAD_ROOT. The maximum size is 10 MiB. Press Enter to upload the file.".into() }, false),
        Dialog::Ask => (" ENTER A QUESTION ", format!("Enter a question about {}. {}", app.active.as_ref().map(|item| item.label.as_str()).unwrap_or("this store"),
            if app.demo { "Demo mode returns a sample answer." } else { "Press Enter to send the question to Gemini." }), false),
        Dialog::Search => (" FILTER DOCUMENTS ", "Enter a document name filter. Press Enter to apply the filter. Press Esc to cancel.".into(), false),
        Dialog::DeleteDocument(item) => (" DELETE DOCUMENT ", format!("Type delete to remove {} and its indexed content.", item.label), true),
        Dialog::DeleteStore(item) => (" DELETE STORE ", format!("Type delete to remove {} and all its documents.", item.label), true),
        Dialog::Quit => (" REQUEST IN PROGRESS ", "A request is not complete. Google can still process the request. Save the operation ID if available. Type quit to close the app.".into(), true),
        Dialog::Help => (" KEYBOARD SHORTCUTS ", String::new(), false),
    };
    let help = matches!(dialog, Dialog::Help);
    let width = area.width.saturating_sub(6).min(78);
    let description_height = Paragraph::new(clean(&description))
        .wrap(Wrap { trim: false })
        .line_count(width.saturating_sub(6));
    let height = if help {
        21
    } else {
        description_height.max(2) as u16 + 10
    }
    .min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    // Reduce the background contrast to make the input field clear.
    frame
        .buffer_mut()
        .set_style(area, Style::default().fg(EDGE));
    frame.render_widget(Clear, popup);
    let block = panel(title, true)
        .border_style(Style::default().fg(if danger { RED } else { TEAL }))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if help {
        let content = vec![
            "Tab / Shift-Tab  Select a panel",
            "↑ ↓ / j k        Select an item or scroll",
            "Enter            Open a store or submit text",
            "n                Create a store",
            "u                Upload a file",
            "a                Enter a question",
            "/                Filter the names",
            "Esc              Close a dialog or clear a filter",
            "r                Reload the list",
            "m                Load the next page",
            "d                Delete a store or document",
            "PgUp / PgDn      Scroll the answer",
            "Ctrl-U           Clear the input field",
            "q / Ctrl-C       Close the app",
            "",
            "Press Enter or Esc to close this list.",
        ];
        frame.render_widget(
            Paragraph::new(content.join("\n"))
                .style(Style::default().fg(TEXT))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    let [description_area, input_area, hints] = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(inner);
    frame.render_widget(
        Paragraph::new(clean(&description))
            .style(Style::default().fg(if danger { GOLD } else { MUTED }))
            .wrap(Wrap { trim: false }),
        description_area,
    );
    let available = input_area.width.saturating_sub(4) as usize;
    let before = &app.input.text[..app.input.cursor];
    // Use display width to keep Unicode characters complete.
    let mut start = before.len();
    let mut used = 0;
    for (index, character) in before.char_indices().rev() {
        let width = Line::from(character.to_string()).width();
        if used + width >= available.max(1) {
            break;
        }
        used += width;
        start = index;
    }
    let visible = &app.input.text[start..];
    frame.render_widget(
        Paragraph::new(visible)
            .style(Style::default().fg(TEXT))
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(if danger { RED } else { TEAL }))
                    .padding(Padding::horizontal(1)),
            ),
        input_area,
    );
    let cursor_x = input_area.x + 2 + Line::from(&before[start..]).width() as u16;
    frame.set_cursor_position((
        cursor_x.min(input_area.right().saturating_sub(2)),
        input_area.y + 1,
    ));
    frame.render_widget(
        Paragraph::new("Enter: Submit   ·   Esc: Cancel   ·   Ctrl-U: Clear")
            .alignment(Alignment::Right)
            .style(Style::default().fg(MUTED)),
        hints,
    );
}
