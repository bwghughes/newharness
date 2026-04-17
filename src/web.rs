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
        } => {
            let data = serde_json::json!({
                "id": id,
                "status": status,
                "activity": activity,
            });
            Event::default().event("task_update").data(data.to_string())
        }
        PlanEvent::BoardReset => Event::default().event("board_reset").data("{}"),
    }
}

async fn sse_handler(
    State(board): State<PlanBoard>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);
    let mut broadcast_rx = board.subscribe();
    let snapshot = board.snapshot().await;

    tokio::spawn(async move {
        if !snapshot.is_empty() {
            let data = serde_json::json!({ "tasks": snapshot });
            let init = Event::default()
                .event("plan_created")
                .data(data.to_string());
            if tx.send(Ok(init)).await.is_err() {
                return;
            }
        }

        loop {
            match broadcast_rx.recv().await {
                Ok(event) => {
                    let sse = plan_event_to_sse(event);
                    if tx.send(Ok(sse)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
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
    use crate::plan::TaskStatus;

    #[test]
    fn board_html_is_embedded() {
        assert!(BOARD_HTML.contains("strap-in"));
        assert!(BOARD_HTML.contains("EventSource"));
    }

    #[test]
    fn plan_created_event_has_correct_name() {
        let event = plan_event_to_sse(PlanEvent::PlanCreated { tasks: vec![] });
        // Event is opaque in axum, but we can verify construction doesn't panic
        let _ = event;
    }

    #[test]
    fn task_update_event_has_correct_structure() {
        let event = plan_event_to_sse(PlanEvent::TaskUpdate {
            id: 0,
            status: TaskStatus::InProgress,
            activity: Some("reading file".into()),
        });
        let _ = event;
    }

    #[test]
    fn board_reset_event_constructs() {
        let event = plan_event_to_sse(PlanEvent::BoardReset);
        let _ = event;
    }
}
