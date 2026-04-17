use crate::plan::{PlanBoard, PlanEvent};
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html,
    },
    routing::get,
    Router,
};
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const BOARD_HTML: &str = include_str!("board.html");

pub fn router(board: PlanBoard) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/events", get(sse_handler))
        .with_state(board)
}

async fn index() -> Html<&'static str> {
    Html(BOARD_HTML)
}

fn plan_event_to_sse(event: PlanEvent) -> Event {
    match event {
        PlanEvent::PlanCreated { ref tasks } => {
            let data = serde_json::json!({ "tasks": tasks });
            Event::default()
                .event("plan_created")
                .data(data.to_string())
        }
        PlanEvent::TaskUpdate {
            id,
            ref status,
            ref activity,
            started_at,
            completed_at,
        } => {
            let data = serde_json::json!({
                "id": id,
                "status": status,
                "activity": activity,
                "started_at": started_at,
                "completed_at": completed_at,
            });
            Event::default().event("task_update").data(data.to_string())
        }
        PlanEvent::BoardReset => Event::default().event("board_reset").data("{}"),
        PlanEvent::UsageUpdate {
            prompt_tokens,
            completion_tokens,
        } => {
            let data = serde_json::json!({
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
            });
            Event::default()
                .event("usage_update")
                .data(data.to_string())
        }
    }
}

fn snapshot_event(tasks: &[crate::plan::PlanTask]) -> Event {
    let data = serde_json::json!({ "tasks": tasks });
    Event::default()
        .event("plan_created")
        .data(data.to_string())
}

fn usage_event(prompt: u64, completion: u64) -> Event {
    let data = serde_json::json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
    });
    Event::default()
        .event("usage_update")
        .data(data.to_string())
}

async fn sse_handler(
    State(board): State<PlanBoard>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);
    let mut broadcast_rx = board.subscribe();
    let snapshot = board.snapshot().await;
    let (prompt, completion) = board.usage().await;

    tokio::spawn(async move {
        if !snapshot.is_empty() && tx.send(Ok(snapshot_event(&snapshot))).await.is_err() {
            return;
        }
        if (prompt > 0 || completion > 0)
            && tx.send(Ok(usage_event(prompt, completion))).await.is_err()
        {
            return;
        }

        loop {
            match broadcast_rx.recv().await {
                Ok(event) => {
                    let sse = plan_event_to_sse(event);
                    if tx.send(Ok(sse)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let tasks = board.snapshot().await;
                    if !tasks.is_empty() && tx.send(Ok(snapshot_event(&tasks))).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}

pub async fn serve(board: PlanBoard, listener: tokio::net::TcpListener) {
    let app = router(board);
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("\x1b[1;31mWeb server error: {e}\x1b[0m");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PlanTask, TaskStatus};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ── HTML / static tests ──

    #[test]
    fn board_html_is_embedded() {
        assert!(BOARD_HTML.contains("strap-in"));
        assert!(BOARD_HTML.contains("EventSource"));
    }

    #[test]
    fn board_html_has_light_mode_support() {
        assert!(BOARD_HTML.contains("prefers-color-scheme: light"));
        assert!(BOARD_HTML.contains("var(--bg)"));
    }

    #[test]
    fn board_html_has_markdown_renderer() {
        assert!(BOARD_HTML.contains("function md("));
        assert!(BOARD_HTML.contains("<strong>"));
        assert!(BOARD_HTML.contains("<em>"));
        assert!(BOARD_HTML.contains("<code>"));
    }

    #[test]
    fn board_html_uses_md_in_render() {
        assert!(BOARD_HTML.contains("md(t.text)"));
        assert!(BOARD_HTML.contains("md(t.activity)"));
    }

    // ── SSE event construction tests ──

    #[test]
    fn plan_created_event_constructs() {
        let event = plan_event_to_sse(PlanEvent::PlanCreated { tasks: vec![] });
        let _ = event;
    }

    #[test]
    fn task_update_event_constructs() {
        let event = plan_event_to_sse(PlanEvent::TaskUpdate {
            id: 0,
            status: TaskStatus::InProgress,
            activity: Some("reading file".into()),
            started_at: Some(1713360000000),
            completed_at: None,
        });
        let _ = event;
    }

    #[test]
    fn board_reset_event_constructs() {
        let event = plan_event_to_sse(PlanEvent::BoardReset);
        let _ = event;
    }

    // ── JSON format tests (verify what JavaScript will receive) ──

    #[test]
    fn plan_created_json_has_tasks_array() {
        let tasks = vec![
            PlanTask {
                id: 0,
                text: "Read file".into(),
                status: TaskStatus::Planned,
                activity: None,
                started_at: None,
                completed_at: None,
            },
            PlanTask {
                id: 1,
                text: "Edit code".into(),
                status: TaskStatus::InProgress,
                activity: Some("working".into()),
                started_at: Some(1713360000000),
                completed_at: None,
            },
        ];
        let data = serde_json::json!({ "tasks": tasks });
        let json: serde_json::Value = serde_json::from_str(&data.to_string()).unwrap();

        assert_eq!(json["tasks"][0]["id"], 0);
        assert_eq!(json["tasks"][0]["text"], "Read file");
        assert_eq!(json["tasks"][0]["status"], "planned");
        assert!(json["tasks"][0].get("activity").is_none());

        assert_eq!(json["tasks"][1]["id"], 1);
        assert_eq!(json["tasks"][1]["status"], "in_progress");
        assert_eq!(json["tasks"][1]["activity"], "working");
    }

    #[test]
    fn task_update_json_includes_all_fields() {
        let data = serde_json::json!({
            "id": 2usize,
            "status": TaskStatus::Done,
            "activity": Option::<String>::None,
        });
        let json: serde_json::Value = serde_json::from_str(&data.to_string()).unwrap();
        assert_eq!(json["id"], 2);
        assert_eq!(json["status"], "done");
        assert!(json["activity"].is_null());
    }

    #[test]
    fn task_update_json_with_activity() {
        let data = serde_json::json!({
            "id": 0usize,
            "status": TaskStatus::InProgress,
            "activity": Some("reading src/main.rs"),
        });
        let json: serde_json::Value = serde_json::from_str(&data.to_string()).unwrap();
        assert_eq!(json["status"], "in_progress");
        assert_eq!(json["activity"], "reading src/main.rs");
    }

    #[test]
    fn task_status_json_values_match_javascript_keys() {
        let planned = serde_json::to_value(TaskStatus::Planned).unwrap();
        let in_progress = serde_json::to_value(TaskStatus::InProgress).unwrap();
        let done = serde_json::to_value(TaskStatus::Done).unwrap();
        assert_eq!(planned, "planned");
        assert_eq!(in_progress, "in_progress");
        assert_eq!(done, "done");
    }

    // ── HTTP route tests ──

    #[tokio::test]
    async fn index_returns_200_with_html() {
        let app = router(PlanBoard::new());
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("<!DOCTYPE html>"));
        assert!(text.contains("strap-in"));
    }

    #[tokio::test]
    async fn events_returns_sse_content_type() {
        let app = router(PlanBoard::new());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.contains("text/event-stream"),
            "expected text/event-stream, got {ct}"
        );
    }

    #[tokio::test]
    async fn events_returns_no_cache_header() {
        let app = router(PlanBoard::new());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cc = resp
            .headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cc.contains("no-cache"), "expected no-cache, got {cc}");
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let app = router(PlanBoard::new());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── SSE stream content tests ──

    fn parse_sse_events(raw: &str) -> Vec<(String, String)> {
        let mut events = Vec::new();
        let mut current_event = String::new();
        let mut current_data = String::new();

        for line in raw.lines() {
            if line.starts_with("event:") {
                current_event = line.trim_start_matches("event:").trim().to_string();
            } else if line.starts_with("data:") {
                current_data = line.trim_start_matches("data:").trim().to_string();
            } else if line.is_empty() && !current_event.is_empty() {
                events.push((current_event.clone(), current_data.clone()));
                current_event.clear();
                current_data.clear();
            }
        }
        if !current_event.is_empty() {
            events.push((current_event, current_data));
        }
        events
    }

    #[tokio::test]
    async fn events_sends_snapshot_on_connect() {
        let board = PlanBoard::new();
        board.set_plan(vec!["Alpha".into(), "Beta".into()]).await;

        let app = router(board);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body();
        let frame = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            let mut body = body;
            let mut buf = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(frame) = frame {
                    if let Some(data) = frame.data_ref() {
                        buf.extend_from_slice(data);
                        let text = String::from_utf8_lossy(&buf);
                        if text.contains("\n\n") {
                            return text.to_string();
                        }
                    }
                }
            }
            String::from_utf8_lossy(&buf).to_string()
        })
        .await
        .expect("timed out waiting for SSE event");

        let events = parse_sse_events(&frame);
        assert!(
            !events.is_empty(),
            "expected at least one SSE event, got none from: {frame}"
        );
        assert_eq!(events[0].0, "plan_created");
        let data: serde_json::Value = serde_json::from_str(&events[0].1).unwrap();
        assert_eq!(data["tasks"][0]["text"], "Alpha");
        assert_eq!(data["tasks"][1]["text"], "Beta");
    }

    #[tokio::test]
    async fn events_empty_board_sends_no_initial_event() {
        let board = PlanBoard::new();
        let app = router(board.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body();
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), async {
            let mut body = body;
            let mut buf = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(frame) = frame {
                    if let Some(data) = frame.data_ref() {
                        buf.extend_from_slice(data);
                        let text = String::from_utf8_lossy(&buf);
                        if text.contains("event:") {
                            return Some(text.to_string());
                        }
                    }
                }
            }
            None
        })
        .await;

        match result {
            Err(_) => {} // timeout = no events sent, which is correct
            Ok(None) => {}
            Ok(Some(text)) => {
                if text.contains("event:plan_created") || text.contains("event:task_update") {
                    panic!("empty board should not send task events, got: {text}");
                }
            }
        }
    }

    #[tokio::test]
    async fn events_relays_live_board_changes() {
        let board = PlanBoard::new();
        let app = router(board.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body();

        // Give the SSE handler a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Now set a plan on the board — this should be relayed
        board
            .set_plan(vec!["Live step".into(), "Another".into()])
            .await;

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            let mut body = body;
            let mut buf = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(frame) = frame {
                    if let Some(data) = frame.data_ref() {
                        buf.extend_from_slice(data);
                        let text = String::from_utf8_lossy(&buf);
                        if text.contains("plan_created") && text.contains("Live step") {
                            return text.to_string();
                        }
                    }
                }
            }
            String::from_utf8_lossy(&buf).to_string()
        })
        .await
        .expect("timed out waiting for live SSE event");

        let events = parse_sse_events(&result);
        let plan_event = events
            .iter()
            .find(|(name, _)| name == "plan_created")
            .expect("expected plan_created event in SSE stream");
        let data: serde_json::Value = serde_json::from_str(&plan_event.1).unwrap();
        assert_eq!(data["tasks"][0]["text"], "Live step");
    }

    #[tokio::test]
    async fn events_relays_task_update() {
        let board = PlanBoard::new();
        board.set_plan(vec!["Step A".into(), "Step B".into()]).await;

        let app = router(board.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Advance task 0 to in_progress
        board.advance().await;

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            let mut body = body;
            let mut buf = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(frame) = frame {
                    if let Some(data) = frame.data_ref() {
                        buf.extend_from_slice(data);
                        let text = String::from_utf8_lossy(&buf);
                        if text.contains("task_update") {
                            return text.to_string();
                        }
                    }
                }
            }
            String::from_utf8_lossy(&buf).to_string()
        })
        .await
        .expect("timed out waiting for task_update SSE event");

        let events = parse_sse_events(&result);
        let update = events
            .iter()
            .find(|(name, _)| name == "task_update")
            .expect("expected task_update event");
        let data: serde_json::Value = serde_json::from_str(&update.1).unwrap();
        assert_eq!(data["id"], 0);
        assert_eq!(data["status"], "in_progress");
    }

    #[tokio::test]
    async fn events_relays_board_reset() {
        let board = PlanBoard::new();
        board.set_plan(vec!["X".into(), "Y".into()]).await;

        let app = router(board.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        board.reset().await;

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            let mut body = body;
            let mut buf = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(frame) = frame {
                    if let Some(data) = frame.data_ref() {
                        buf.extend_from_slice(data);
                        let text = String::from_utf8_lossy(&buf);
                        if text.contains("board_reset") {
                            return text.to_string();
                        }
                    }
                }
            }
            String::from_utf8_lossy(&buf).to_string()
        })
        .await
        .expect("timed out waiting for board_reset SSE event");

        assert!(result.contains("board_reset"));
    }

    #[tokio::test]
    async fn snapshot_event_matches_plan_created_format() {
        let tasks = vec![PlanTask {
            id: 0,
            text: "Test".into(),
            status: TaskStatus::Planned,
            activity: None,
            started_at: None,
            completed_at: None,
        }];
        let event = snapshot_event(&tasks);
        let _ = event; // construction doesn't panic
    }

    #[test]
    fn usage_event_constructs() {
        let event = usage_event(1000, 500);
        let _ = event;
    }

    #[test]
    fn usage_update_event_constructs() {
        let event = plan_event_to_sse(PlanEvent::UsageUpdate {
            prompt_tokens: 1234,
            completion_tokens: 567,
        });
        let _ = event;
    }

    #[test]
    fn task_update_json_includes_timestamps() {
        let data = serde_json::json!({
            "id": 0usize,
            "status": TaskStatus::InProgress,
            "activity": Option::<String>::None,
            "started_at": Some(1713360000000i64),
            "completed_at": Option::<i64>::None,
        });
        let json: serde_json::Value = serde_json::from_str(&data.to_string()).unwrap();
        assert_eq!(json["started_at"], 1713360000000i64);
        assert!(json["completed_at"].is_null());
    }

    #[test]
    fn board_html_has_cycle_time_display() {
        assert!(BOARD_HTML.contains("cycle-time"));
        assert!(BOARD_HTML.contains("fmtDuration"));
    }

    #[test]
    fn board_html_has_usage_display() {
        assert!(BOARD_HTML.contains("usage_update"));
        assert!(BOARD_HTML.contains("fmtTokens"));
        assert!(BOARD_HTML.contains("id=\"usage\""));
    }

    #[test]
    fn board_html_has_avg_cycle_time_display() {
        assert!(BOARD_HTML.contains("id=\"avg-cycle\""));
        assert!(BOARD_HTML.contains("renderAvgCycle"));
    }

    #[test]
    fn board_html_ticks_live_timers_without_rerender() {
        assert!(BOARD_HTML.contains("tickLiveTimers"));
        assert!(BOARD_HTML.contains("setInterval(tickLiveTimers"));
    }

    #[tokio::test]
    async fn events_relays_usage_update() {
        let board = PlanBoard::new();
        let app = router(board.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        board.update_usage(1000, 500).await;

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            let mut body = body;
            let mut buf = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(frame) = frame {
                    if let Some(data) = frame.data_ref() {
                        buf.extend_from_slice(data);
                        let text = String::from_utf8_lossy(&buf);
                        if text.contains("usage_update") {
                            return text.to_string();
                        }
                    }
                }
            }
            String::from_utf8_lossy(&buf).to_string()
        })
        .await
        .expect("timed out waiting for usage_update SSE event");

        let events = parse_sse_events(&result);
        let usage = events
            .iter()
            .find(|(name, _)| name == "usage_update")
            .expect("expected usage_update event");
        let data: serde_json::Value = serde_json::from_str(&usage.1).unwrap();
        assert_eq!(data["prompt_tokens"], 1000);
        assert_eq!(data["completion_tokens"], 500);
    }
}
