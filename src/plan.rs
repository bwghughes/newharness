use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Planned,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanTask {
    pub id: usize,
    pub text: String,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum PlanEvent {
    PlanCreated {
        tasks: Vec<PlanTask>,
    },
    TaskUpdate {
        id: usize,
        status: TaskStatus,
        activity: Option<String>,
        started_at: Option<i64>,
        completed_at: Option<i64>,
    },
    BoardReset,
    UsageUpdate {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
}

#[derive(Debug, Default)]
struct BoardState {
    tasks: Vec<PlanTask>,
    current_step: Option<usize>,
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct PlanBoard {
    state: Arc<RwLock<BoardState>>,
    tx: broadcast::Sender<PlanEvent>,
}

fn epoch_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl PlanBoard {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            state: Arc::new(RwLock::new(BoardState::default())),
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PlanEvent> {
        self.tx.subscribe()
    }

    pub async fn snapshot(&self) -> Vec<PlanTask> {
        self.state.read().await.tasks.clone()
    }

    pub async fn set_plan(&self, texts: Vec<String>) {
        let tasks: Vec<PlanTask> = texts
            .into_iter()
            .enumerate()
            .map(|(id, text)| PlanTask {
                id,
                text,
                status: TaskStatus::Planned,
                activity: None,
                started_at: None,
                completed_at: None,
            })
            .collect();

        let mut state = self.state.write().await;
        state.tasks = tasks.clone();
        state.current_step = None;
        let _ = self.tx.send(PlanEvent::PlanCreated { tasks });
    }

    pub async fn advance(&self) {
        let mut state = self.state.write().await;
        let now = epoch_millis();

        if let Some(current) = state.current_step {
            if let Some(task) = state.tasks.get_mut(current) {
                task.status = TaskStatus::Done;
                task.completed_at = Some(now);
                task.activity = None;
                let _ = self.tx.send(PlanEvent::TaskUpdate {
                    id: current,
                    status: TaskStatus::Done,
                    activity: None,
                    started_at: task.started_at,
                    completed_at: task.completed_at,
                });
            }
        }

        let next = state
            .tasks
            .iter()
            .position(|t| t.status == TaskStatus::Planned);
        if let Some(next_id) = next {
            state.tasks[next_id].status = TaskStatus::InProgress;
            state.tasks[next_id].started_at = Some(now);
            state.current_step = Some(next_id);
            let _ = self.tx.send(PlanEvent::TaskUpdate {
                id: next_id,
                status: TaskStatus::InProgress,
                activity: None,
                started_at: Some(now),
                completed_at: None,
            });
        } else {
            state.current_step = None;
        }
    }

    pub async fn update_activity(&self, activity: &str) {
        let mut state = self.state.write().await;
        if let Some(current) = state.current_step {
            if let Some(task) = state.tasks.get_mut(current) {
                task.activity = Some(activity.to_string());
                let _ = self.tx.send(PlanEvent::TaskUpdate {
                    id: current,
                    status: TaskStatus::InProgress,
                    activity: Some(activity.to_string()),
                    started_at: task.started_at,
                    completed_at: None,
                });
            }
        }
    }

    pub async fn complete_current(&self) {
        let mut state = self.state.write().await;
        if let Some(current) = state.current_step {
            if let Some(task) = state.tasks.get_mut(current) {
                task.status = TaskStatus::Done;
                task.completed_at = Some(epoch_millis());
                task.activity = None;
                let _ = self.tx.send(PlanEvent::TaskUpdate {
                    id: current,
                    status: TaskStatus::Done,
                    activity: None,
                    started_at: task.started_at,
                    completed_at: task.completed_at,
                });
            }
            state.current_step = None;
        }
    }

    pub async fn update_usage(&self, prompt: u64, completion: u64) {
        let mut state = self.state.write().await;
        state.prompt_tokens += prompt;
        state.completion_tokens += completion;
        let _ = self.tx.send(PlanEvent::UsageUpdate {
            prompt_tokens: state.prompt_tokens,
            completion_tokens: state.completion_tokens,
        });
    }

    pub async fn usage(&self) -> (u64, u64) {
        let state = self.state.read().await;
        (state.prompt_tokens, state.completion_tokens)
    }

    pub async fn reset(&self) {
        let mut state = self.state.write().await;
        state.tasks.clear();
        state.current_step = None;
        let _ = self.tx.send(PlanEvent::BoardReset);
    }
}

/// Parse a numbered plan from agent text output.
/// Expects consecutive numbered lines like "1. Do something", "2) Do another".
/// Returns None if fewer than 2 steps are found.
pub fn parse_plan(text: &str) -> Option<Vec<String>> {
    let mut steps = Vec::new();
    let mut last_num = 0;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some((num_str, rest)) = split_numbered_line(trimmed) {
            if let Ok(num) = num_str.parse::<usize>() {
                if num == last_num + 1 {
                    steps.push(rest.to_string());
                    last_num = num;
                }
            }
        }
    }

    if steps.len() >= 2 {
        Some(steps)
    } else {
        None
    }
}

const TASKS_FILE: &str = "TASKS.md";
const TASKS_HEADER: &str =
    "# Tasks\n\n<!-- strap-in automatically appends the plan and completion state here -->\n";

/// Append a timestamped entry capturing the full state of the board's plan
/// to `TASKS.md` in the given workdir. Creates the file with a header if it
/// doesn't exist. Every task in the plan is recorded with its final status
/// (`[x]` for done, `[ ]` for still-planned or in-progress). A trailing note
/// is added when in-progress tasks remain. Skips only when the board has
/// no tasks at all.
pub async fn write_history(
    board: &PlanBoard,
    workdir: &Path,
    user_input: &str,
) -> std::io::Result<()> {
    let snapshot = board.snapshot().await;
    if snapshot.is_empty() {
        return Ok(());
    }

    let done_count = snapshot
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();
    let total = snapshot.len();

    let path = workdir.join(TASKS_FILE);
    let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let request_line = user_input.replace(['\n', '\r'], " ");

    let mut entry = String::new();
    entry.push_str("\n## ");
    entry.push_str(&ts.to_string());
    entry.push_str(&format!(" — {done_count}/{total} complete"));
    entry.push_str("\n\n**Request:** ");
    entry.push_str(request_line.trim());
    entry.push_str("\n\n");
    for task in &snapshot {
        let mark = if task.status == TaskStatus::Done {
            "[x]"
        } else {
            "[ ]"
        };
        entry.push_str("- ");
        entry.push_str(mark);
        entry.push(' ');
        entry.push_str(&task.text);
        if task.status == TaskStatus::InProgress {
            entry.push_str(" _(in progress)_");
        }
        if let (Some(start), Some(end)) = (task.started_at, task.completed_at) {
            let secs = (end - start) / 1000;
            if secs >= 60 {
                entry.push_str(&format!(" _{}m{}s_", secs / 60, secs % 60));
            } else {
                entry.push_str(&format!(" _{secs}s_"));
            }
        }
        entry.push('\n');
    }
    entry.push_str("\n---\n");

    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    if !file_exists {
        file.write_all(TASKS_HEADER.as_bytes()).await?;
    }
    file.write_all(entry.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

fn split_numbered_line(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }

    if i == 0 || i >= bytes.len() {
        return None;
    }

    let num = &line[..i];

    let sep = bytes[i];
    if sep != b'.' && sep != b')' && sep != b':' {
        return None;
    }
    i += 1;

    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }

    if i >= bytes.len() {
        return None;
    }

    Some((num, &line[i..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_plan tests ──

    #[test]
    fn parse_plan_dot_format() {
        let text = "Here is my plan:\n1. Read the file\n2. Edit the code\n3. Run tests";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps, vec!["Read the file", "Edit the code", "Run tests"]);
    }

    #[test]
    fn parse_plan_paren_format() {
        let text = "1) First step\n2) Second step";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps, vec!["First step", "Second step"]);
    }

    #[test]
    fn parse_plan_colon_format() {
        let text = "1: Alpha\n2: Beta\n3: Gamma";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps, vec!["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn parse_plan_ignores_preamble() {
        let text = "I'll work through this step by step:\n\n1. Explore the codebase\n2. Write tests\n3. Implement feature\n\nLet's begin.";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0], "Explore the codebase");
    }

    #[test]
    fn parse_plan_returns_none_for_single_step() {
        let text = "1. Only one step";
        assert!(parse_plan(text).is_none());
    }

    #[test]
    fn parse_plan_returns_none_for_no_steps() {
        let text = "Just some regular text without any numbered list.";
        assert!(parse_plan(text).is_none());
    }

    #[test]
    fn parse_plan_skips_non_consecutive() {
        let text = "1. First\n3. Third\n5. Fifth";
        assert!(parse_plan(text).is_none());
    }

    #[test]
    fn parse_plan_handles_indented_lines() {
        let text = "  1. Step one\n  2. Step two";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps, vec!["Step one", "Step two"]);
    }

    #[test]
    fn parse_plan_with_bold_markdown() {
        let text =
            "1. **RED** Write a failing test\n2. **GREEN** Make it pass\n3. **REFACTOR** Clean up";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps[0], "**RED** Write a failing test");
    }

    #[test]
    fn split_numbered_line_basic() {
        let (num, rest) = split_numbered_line("1. Hello").unwrap();
        assert_eq!(num, "1");
        assert_eq!(rest, "Hello");
    }

    #[test]
    fn split_numbered_line_multi_digit() {
        let (num, rest) = split_numbered_line("12. Twelfth step").unwrap();
        assert_eq!(num, "12");
        assert_eq!(rest, "Twelfth step");
    }

    #[test]
    fn split_numbered_line_no_separator() {
        assert!(split_numbered_line("1 Hello").is_none());
    }

    #[test]
    fn split_numbered_line_no_text_after() {
        assert!(split_numbered_line("1.").is_none());
    }

    #[test]
    fn split_numbered_line_empty() {
        assert!(split_numbered_line("").is_none());
    }

    #[test]
    fn split_numbered_line_no_digits() {
        assert!(split_numbered_line("abc. def").is_none());
    }

    // ── PlanBoard tests ──

    #[tokio::test]
    async fn board_starts_empty() {
        let board = PlanBoard::new();
        assert!(board.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn board_set_plan_creates_tasks() {
        let board = PlanBoard::new();
        board.set_plan(vec!["Step A".into(), "Step B".into()]).await;
        let tasks = board.snapshot().await;
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].text, "Step A");
        assert_eq!(tasks[0].status, TaskStatus::Planned);
        assert_eq!(tasks[1].text, "Step B");
    }

    #[tokio::test]
    async fn board_advance_moves_first_to_in_progress() {
        let board = PlanBoard::new();
        board
            .set_plan(vec!["A".into(), "B".into(), "C".into()])
            .await;
        board.advance().await;
        let tasks = board.snapshot().await;
        assert_eq!(tasks[0].status, TaskStatus::InProgress);
        assert_eq!(tasks[1].status, TaskStatus::Planned);
        assert_eq!(tasks[2].status, TaskStatus::Planned);
    }

    #[tokio::test]
    async fn board_advance_twice_completes_first_starts_second() {
        let board = PlanBoard::new();
        board
            .set_plan(vec!["A".into(), "B".into(), "C".into()])
            .await;
        board.advance().await;
        board.advance().await;
        let tasks = board.snapshot().await;
        assert_eq!(tasks[0].status, TaskStatus::Done);
        assert_eq!(tasks[1].status, TaskStatus::InProgress);
        assert_eq!(tasks[2].status, TaskStatus::Planned);
    }

    #[tokio::test]
    async fn board_complete_current_marks_done() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into(), "B".into()]).await;
        board.advance().await;
        board.complete_current().await;
        let tasks = board.snapshot().await;
        assert_eq!(tasks[0].status, TaskStatus::Done);
        assert_eq!(tasks[1].status, TaskStatus::Planned);
    }

    #[tokio::test]
    async fn board_update_activity_sets_text() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into()]).await;
        board.advance().await;
        board.update_activity("reading src/main.rs").await;
        let tasks = board.snapshot().await;
        assert_eq!(tasks[0].activity.as_deref(), Some("reading src/main.rs"));
    }

    #[tokio::test]
    async fn board_advance_past_end_is_noop() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into()]).await;
        board.advance().await;
        board.advance().await; // completes A, no more planned
        let tasks = board.snapshot().await;
        assert_eq!(tasks[0].status, TaskStatus::Done);
    }

    #[tokio::test]
    async fn board_reset_clears_everything() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into(), "B".into()]).await;
        board.advance().await;
        board.reset().await;
        assert!(board.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn board_broadcast_sends_events() {
        let board = PlanBoard::new();
        let mut rx = board.subscribe();
        board.set_plan(vec!["X".into(), "Y".into()]).await;

        let event = rx.try_recv().unwrap();
        assert!(matches!(event, PlanEvent::PlanCreated { tasks } if tasks.len() == 2));
    }

    #[tokio::test]
    async fn board_advance_broadcasts_two_events() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into(), "B".into()]).await;
        let mut rx = board.subscribe();
        board.advance().await;
        // First advance: just starts task 0 (no previous to complete)
        let event = rx.try_recv().unwrap();
        assert!(matches!(
            event,
            PlanEvent::TaskUpdate {
                id: 0,
                status: TaskStatus::InProgress,
                ..
            }
        ));
    }

    #[test]
    fn task_status_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&TaskStatus::InProgress).unwrap(),
            r#""in_progress""#
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Planned).unwrap(),
            r#""planned""#
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Done).unwrap(),
            r#""done""#
        );
    }

    #[test]
    fn plan_task_serializes_correctly() {
        let task = PlanTask {
            id: 0,
            text: "Do stuff".into(),
            status: TaskStatus::InProgress,
            activity: Some("reading file".into()),
            started_at: Some(1713360000000),
            completed_at: None,
        };
        let json: serde_json::Value = serde_json::to_value(&task).unwrap();
        assert_eq!(json["id"], 0);
        assert_eq!(json["text"], "Do stuff");
        assert_eq!(json["status"], "in_progress");
        assert_eq!(json["activity"], "reading file");
        assert_eq!(json["started_at"], 1713360000000i64);
        assert!(json.get("completed_at").is_none());
    }

    // ── write_history tests ──

    #[tokio::test]
    async fn write_history_creates_file_with_header_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        board.set_plan(vec!["First".into(), "Second".into()]).await;
        board.advance().await;
        board.advance().await;
        board.complete_current().await;

        write_history(&board, dir.path(), "Make a thing")
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        assert!(content.starts_with("# Tasks"));
        assert!(content.contains("strap-in automatically appends"));
        assert!(content.contains("**Request:** Make a thing"));
        assert!(content.contains("- [x] First"));
        assert!(content.contains("- [x] Second"));
        assert!(content.contains("2/2 complete"));
    }

    #[tokio::test]
    async fn write_history_appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("TASKS.md");
        tokio::fs::write(&path, "# Tasks\n\nExisting stuff\n")
            .await
            .unwrap();

        let board = PlanBoard::new();
        board.set_plan(vec!["Added".into(), "Later".into()]).await;
        board.advance().await;
        board.advance().await;
        board.complete_current().await;

        write_history(&board, dir.path(), "New request")
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("Existing stuff"));
        assert!(content.contains("**Request:** New request"));
        assert!(content.contains("- [x] Added"));
        // Header was not duplicated
        assert_eq!(content.matches("# Tasks\n").count(), 1);
    }

    #[tokio::test]
    async fn write_history_skips_only_when_board_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        // Board has no tasks at all — nothing to log
        write_history(&board, dir.path(), "Nothing happened")
            .await
            .unwrap();

        let exists = tokio::fs::try_exists(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        assert!(
            !exists,
            "TASKS.md should not be created when the board is empty"
        );
    }

    #[tokio::test]
    async fn write_history_includes_planned_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        board
            .set_plan(vec!["Planned A".into(), "Planned B".into()])
            .await;
        // No advance — everything stays Planned

        write_history(&board, dir.path(), "Planning only")
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        assert!(content.contains("**Request:** Planning only"));
        assert!(content.contains("- [ ] Planned A"));
        assert!(content.contains("- [ ] Planned B"));
        assert!(content.contains("0/2 complete"));
        assert!(!content.contains("- [x]"));
    }

    #[tokio::test]
    async fn write_history_mixes_done_and_planned() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        board
            .set_plan(vec!["First".into(), "Second".into(), "Third".into()])
            .await;
        board.advance().await;
        board.complete_current().await;
        // First is Done; Second and Third still Planned

        write_history(&board, dir.path(), "Partial work")
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        assert!(content.contains("- [x] First"));
        assert!(content.contains("- [ ] Second"));
        assert!(content.contains("- [ ] Third"));
        assert!(content.contains("1/3 complete"));
    }

    #[tokio::test]
    async fn write_history_marks_in_progress_with_note() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        board
            .set_plan(vec!["Running".into(), "Queued".into()])
            .await;
        board.advance().await;
        // Task 0 is now InProgress, task 1 is Planned

        write_history(&board, dir.path(), "Mid-flight")
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        assert!(content.contains("- [ ] Running _(in progress)_"));
        assert!(content.contains("- [ ] Queued\n"));
        assert!(content.contains("0/2 complete"));
    }

    #[tokio::test]
    async fn write_history_preserves_task_order() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        board
            .set_plan(vec!["Alpha".into(), "Bravo".into(), "Charlie".into()])
            .await;

        write_history(&board, dir.path(), "Ordering test")
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        let a = content.find("Alpha").unwrap();
        let b = content.find("Bravo").unwrap();
        let c = content.find("Charlie").unwrap();
        assert!(a < b && b < c, "tasks should appear in plan order");
    }

    #[tokio::test]
    async fn write_history_flattens_multiline_request() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        board.set_plan(vec!["Step".into()]).await;
        board.advance().await;
        board.complete_current().await;

        write_history(&board, dir.path(), "Line one\nLine two\nLine three")
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        assert!(content.contains("**Request:** Line one Line two Line three"));
    }

    #[tokio::test]
    async fn write_history_contains_utc_timestamp_format() {
        let dir = tempfile::tempdir().unwrap();
        let board = PlanBoard::new();
        board.set_plan(vec!["x".into()]).await;
        board.advance().await;
        board.complete_current().await;

        write_history(&board, dir.path(), "r").await.unwrap();

        let content = tokio::fs::read_to_string(dir.path().join("TASKS.md"))
            .await
            .unwrap();
        // Heading includes "YYYY-MM-DD HH:MM:SS UTC" followed by completion ratio
        let re = content
            .lines()
            .any(|l| l.starts_with("## ") && l.contains(" UTC") && l.contains("1/1 complete"));
        assert!(
            re,
            "expected a ## <timestamp> UTC — N/M complete heading, got:\n{content}"
        );
    }

    #[tokio::test]
    async fn advance_sets_started_at_timestamp() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into(), "B".into()]).await;
        board.advance().await;
        let tasks = board.snapshot().await;
        assert!(
            tasks[0].started_at.is_some(),
            "in-progress task should have started_at"
        );
        assert!(tasks[0].completed_at.is_none());
        assert!(tasks[1].started_at.is_none());
    }

    #[tokio::test]
    async fn advance_sets_completed_at_on_done() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into(), "B".into()]).await;
        board.advance().await;
        board.advance().await;
        let tasks = board.snapshot().await;
        assert!(tasks[0].started_at.is_some());
        assert!(tasks[0].completed_at.is_some());
        assert!(
            tasks[0].completed_at.unwrap() >= tasks[0].started_at.unwrap(),
            "completed_at should be >= started_at"
        );
        assert!(tasks[1].started_at.is_some());
        assert!(tasks[1].completed_at.is_none());
    }

    #[tokio::test]
    async fn complete_current_sets_completed_at() {
        let board = PlanBoard::new();
        board.set_plan(vec!["A".into()]).await;
        board.advance().await;
        board.complete_current().await;
        let tasks = board.snapshot().await;
        assert!(tasks[0].completed_at.is_some());
    }

    #[tokio::test]
    async fn update_usage_accumulates() {
        let board = PlanBoard::new();
        board.update_usage(100, 50).await;
        board.update_usage(200, 75).await;
        let (prompt, completion) = board.usage().await;
        assert_eq!(prompt, 300);
        assert_eq!(completion, 125);
    }

    #[tokio::test]
    async fn usage_broadcasts_event() {
        let board = PlanBoard::new();
        let mut rx = board.subscribe();
        board.update_usage(100, 50).await;
        let event = rx.try_recv().unwrap();
        assert!(matches!(
            event,
            PlanEvent::UsageUpdate {
                prompt_tokens: 100,
                completion_tokens: 50,
            }
        ));
    }

    #[tokio::test]
    async fn reset_preserves_usage() {
        let board = PlanBoard::new();
        board.update_usage(500, 200).await;
        board.reset().await;
        let (prompt, completion) = board.usage().await;
        assert_eq!(prompt, 500);
        assert_eq!(completion, 200);
    }

    #[test]
    fn plan_task_omits_null_activity() {
        let task = PlanTask {
            id: 0,
            text: "X".into(),
            status: TaskStatus::Planned,
            activity: None,
            started_at: None,
            completed_at: None,
        };
        let json: serde_json::Value = serde_json::to_value(&task).unwrap();
        assert!(json.get("activity").is_none());
        assert!(json.get("started_at").is_none());
        assert!(json.get("completed_at").is_none());
    }
}
