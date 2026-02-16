use std::sync::Arc;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Row, Table};

use super::format_tokens;
use crate::metrics::{MetricsStore, RequestRecord};

/// Builds model-summary rows from a snapshot. Shared by the Models tab and the
/// overview Token Usage panel.
pub fn model_table(snap: &[RequestRecord], title: String, skip: usize) -> (Table<'static>, usize) {
    let groups = MetricsStore::group_by(snap, |r| r.model.clone());

    let header = Row::new(vec![
        "", "Model", "Reqs", "In", "Out", "Avg/Req", "P50", "P95", "Errs",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let mut model_names: Vec<String> = groups.keys().cloned().collect();
    model_names.sort();
    let total = model_names.len();

    let rows: Vec<Row> = model_names
        .iter()
        .skip(skip)
        .map(|model| {
            let records = &groups[model];
            let count = records.len() as u64;
            let input: u64 = records.iter().map(|r| r.input_tokens).sum();
            let output: u64 = records.iter().map(|r| r.output_tokens).sum();
            let durations: Vec<_> = records.iter().map(|r| r.duration).collect();
            let p50 = MetricsStore::duration_percentile(&durations, 50);
            let p95 = MetricsStore::duration_percentile(&durations, 95);
            let errors: u64 = records.iter().filter(|r| r.status >= 400).count() as u64;
            let routed = records.iter().any(|r| r.routed);

            let indicator = if routed { "->" } else { "" };

            let error_style = if errors > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            Row::new(vec![
                Cell::from(indicator).style(Style::default().fg(Color::Yellow)),
                Cell::from(model.clone()).style(Style::default().fg(Color::White)),
                Cell::from(format_tokens(count)),
                Cell::from(format_tokens(input)).style(Style::default().fg(Color::Cyan)),
                Cell::from(format_tokens(output)).style(Style::default().fg(Color::Green)),
                Cell::from(format_tokens((input + output) / count.max(1)))
                    .style(Style::default().fg(Color::White)),
                Cell::from(format!("{}ms", p50.as_millis())),
                Cell::from(format!("{}ms", p95.as_millis())),
                Cell::from(format_tokens(errors)).style(error_style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Min(25),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(title));

    (table, total)
}

pub fn draw(frame: &mut Frame, area: Rect, metrics: &Arc<MetricsStore>, scroll: usize) {
    let snap = metrics.snapshot();
    let (table, total) = model_table(&snap, " Models ".to_string(), scroll);
    frame.render_widget(table, area);
    super::render_scrollbar(frame, area, total, scroll);
}
