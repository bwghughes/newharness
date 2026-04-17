use serde::Serialize;
use std::sync::Arc;
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
    },
    BoardReset,
}

#[derive(Debug, Default)]
struct BoardState {
    tasks: Vec<PlanTask>,
    current_step: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct PlanBoard {
    state: Arc<RwLock<BoardState>>,
    tx: broadcast::Sender<PlanEvent>,
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
            })
            .collect();

        let mut state = self.state.write().await;
        state.tasks = tasks.clone();
        state.current_step = None;
        let _ = self.tx.send(PlanEvent::PlanCreated { tasks });
    }

    pub async fn advance(&self) {
        let mut state = self.state.write().await;

        if let Some(current) = state.current_step {
            if let Some(task) = state.tasks.get_mut(current) {
                task.status = TaskStatus::Done;
                task.activity = None;
                let _ = self.tx.send(PlanEvent::TaskUpdate {
                    id: current,
                    status: TaskStatus::Done,
                    activity: None,
                });
            }
        }

        let next = state
            .tasks
            .iter()
            .position(|t| t.status == TaskStatus::Planned);
        if let Some(next_id) = next {
            state.tasks[next_id].status = TaskStatus::InProgress;
            state.current_step = Some(next_id);
            let _ = self.tx.send(PlanEvent::TaskUpdate {
                id: next_id,
                status: TaskStatus::InProgress,
                activity: None,
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
                });
            }
        }
    }

    pub async fn complete_current(&self) {
        let mut state = self.state.write().await;
        if let Some(current) = state.current_step {
            if let Some(task) = state.tasks.get_mut(current) {
                task.status = TaskStatus::Done;
                task.activity = None;
                let _ = self.tx.send(PlanEvent::TaskUpdate {
                    id: current,
                    status: TaskStatus::Done,
                    activity: None,
                });
            }
            state.current_step = None;
        }
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
        };
        let json: serde_json::Value = serde_json::to_value(&task).unwrap();
        assert_eq!(json["id"], 0);
        assert_eq!(json["text"], "Do stuff");
        assert_eq!(json["status"], "in_progress");
        assert_eq!(json["activity"], "reading file");
    }

    #[test]
    fn plan_task_omits_null_activity() {
        let task = PlanTask {
            id: 0,
            text: "X".into(),
            status: TaskStatus::Planned,
            activity: None,
        };
        let json: serde_json::Value = serde_json::to_value(&task).unwrap();
        assert!(json.get("activity").is_none());
    }
}
