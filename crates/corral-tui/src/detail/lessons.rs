use corral_core::Task;
use corral_ops::Ctx;
use ratatui::text::{Line, Span};

use crate::{artifacts::ui, theme};

pub(super) fn lines(ctx: &Ctx, task: &Task) -> Vec<Line<'static>> {
    let Some(run) = task.current_run() else {
        return vec![];
    };
    let mut lines = vec![Line::default()];
    match corral_ops::applied_lessons(ctx, task, run) {
        Ok(Some(lessons)) => {
            lines.push(Line::from(Span::styled(
                format!("Lessons applied - {} ({})", lessons.len(), run.id),
                theme::bold(),
            )));
            for lesson in lessons {
                lines.extend(ui::text_lines(&format!("{}: {}", lesson.id, lesson.rule)));
                for evidence in lesson.evidence {
                    lines.push(Line::from(Span::styled(
                        format!("  {} - {}", evidence.task, evidence.source),
                        theme::dim(),
                    )));
                }
            }
        }
        Ok(None) => lines.push(Line::from(Span::styled(
            "Lessons applied: not recorded for this run",
            theme::dim(),
        ))),
        Err(error) => lines.push(Line::from(Span::styled(
            format!("Could not read applied lessons: {error:#}"),
            theme::dim(),
        ))),
    }
    lines
}
