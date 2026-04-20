use serde::Serialize;
use std::sync::{Arc, OnceLock};
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Subagent {
    pub id: String,
    pub task: String,
    pub status: SubagentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SubagentEvent {
    Created(Subagent),
    Updated(Subagent),
}

#[derive(Default)]
struct Inner {
    agents: Vec<Subagent>,
    counter: u64,
}

#[derive(Clone)]
pub struct SubagentRegistry {
    inner: Arc<RwLock<Inner>>,
    tx: broadcast::Sender<SubagentEvent>,
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl SubagentRegistry {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(RwLock::new(Inner::default())),
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SubagentEvent> {
        self.tx.subscribe()
    }

    pub async fn snapshot(&self) -> Vec<Subagent> {
        self.inner.read().await.agents.clone()
    }

    pub async fn register(&self, task: String) -> String {
        let mut inner = self.inner.write().await;
        inner.counter += 1;
        let id = format!("sub-{}", inner.counter);
        let agent = Subagent {
            id: id.clone(),
            task,
            status: SubagentStatus::Running,
            activity: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            started_at: now_millis(),
            completed_at: None,
            result: None,
        };
        inner.agents.push(agent.clone());
        let _ = self.tx.send(SubagentEvent::Created(agent));
        id
    }

    pub async fn update_activity(&self, id: &str, activity: String) {
        let mut inner = self.inner.write().await;
        if let Some(a) = inner.agents.iter_mut().find(|a| a.id == id) {
            a.activity = Some(activity);
            let _ = self.tx.send(SubagentEvent::Updated(a.clone()));
        }
    }

    pub async fn set_usage(&self, id: &str, prompt: u64, completion: u64) {
        let mut inner = self.inner.write().await;
        if let Some(a) = inner.agents.iter_mut().find(|a| a.id == id) {
            a.prompt_tokens = prompt;
            a.completion_tokens = completion;
            let _ = self.tx.send(SubagentEvent::Updated(a.clone()));
        }
    }

    pub async fn complete(&self, id: &str, status: SubagentStatus, result: Option<String>) {
        let mut inner = self.inner.write().await;
        if let Some(a) = inner.agents.iter_mut().find(|a| a.id == id) {
            a.status = status;
            a.completed_at = Some(now_millis());
            a.result = result;
            a.activity = None;
            let _ = self.tx.send(SubagentEvent::Updated(a.clone()));
        }
    }
}

pub fn registry() -> &'static SubagentRegistry {
    static REGISTRY: OnceLock<SubagentRegistry> = OnceLock::new();
    REGISTRY.get_or_init(SubagentRegistry::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_starts_empty() {
        let r = SubagentRegistry::new();
        assert!(r.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn register_adds_running_agent() {
        let r = SubagentRegistry::new();
        let id = r.register("do a thing".into()).await;
        let snap = r.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, id);
        assert_eq!(snap[0].task, "do a thing");
        assert_eq!(snap[0].status, SubagentStatus::Running);
    }

    #[tokio::test]
    async fn register_assigns_unique_ids() {
        let r = SubagentRegistry::new();
        let a = r.register("a".into()).await;
        let b = r.register("b".into()).await;
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn update_activity_sets_text() {
        let r = SubagentRegistry::new();
        let id = r.register("work".into()).await;
        r.update_activity(&id, "reading foo.rs".into()).await;
        let snap = r.snapshot().await;
        assert_eq!(snap[0].activity.as_deref(), Some("reading foo.rs"));
    }

    #[tokio::test]
    async fn set_usage_overwrites_values() {
        let r = SubagentRegistry::new();
        let id = r.register("x".into()).await;
        r.set_usage(&id, 100, 50).await;
        r.set_usage(&id, 200, 75).await;
        let snap = r.snapshot().await;
        assert_eq!(snap[0].prompt_tokens, 200);
        assert_eq!(snap[0].completion_tokens, 75);
    }

    #[tokio::test]
    async fn complete_marks_done() {
        let r = SubagentRegistry::new();
        let id = r.register("x".into()).await;
        r.complete(&id, SubagentStatus::Completed, Some("out".into())).await;
        let snap = r.snapshot().await;
        assert_eq!(snap[0].status, SubagentStatus::Completed);
        assert_eq!(snap[0].result.as_deref(), Some("out"));
        assert!(snap[0].completed_at.is_some());
        assert!(snap[0].activity.is_none());
    }

    #[tokio::test]
    async fn subscribe_receives_created_event() {
        let r = SubagentRegistry::new();
        let mut rx = r.subscribe();
        r.register("foo".into()).await;
        let ev = rx.try_recv().unwrap();
        match ev {
            SubagentEvent::Created(a) => assert_eq!(a.task, "foo"),
            _ => panic!("expected Created"),
        }
    }

    #[tokio::test]
    async fn subscribe_receives_updated_event() {
        let r = SubagentRegistry::new();
        let id = r.register("foo".into()).await;
        let mut rx = r.subscribe();
        r.update_activity(&id, "doing stuff".into()).await;
        let ev = rx.try_recv().unwrap();
        match ev {
            SubagentEvent::Updated(a) => assert_eq!(a.activity.as_deref(), Some("doing stuff")),
            _ => panic!("expected Updated"),
        }
    }

    #[test]
    fn global_registry_is_singleton() {
        let a = registry();
        let b = registry();
        // Arc-based equality on inner pointer
        assert!(Arc::ptr_eq(&a.inner, &b.inner));
    }

    #[test]
    fn subagent_status_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&SubagentStatus::Running).unwrap(), r#""running""#);
        assert_eq!(serde_json::to_string(&SubagentStatus::Completed).unwrap(), r#""completed""#);
        assert_eq!(serde_json::to_string(&SubagentStatus::Failed).unwrap(), r#""failed""#);
    }

    #[test]
    fn subagent_omits_optional_nulls() {
        let a = Subagent {
            id: "x".into(),
            task: "t".into(),
            status: SubagentStatus::Running,
            activity: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            started_at: 1,
            completed_at: None,
            result: None,
        };
        let json: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert!(json.get("activity").is_none());
        assert!(json.get("completed_at").is_none());
        assert!(json.get("result").is_none());
    }
}
