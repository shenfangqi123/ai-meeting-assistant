use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct UiEventEnvelope {
    pub name: String,
    pub payload: Value,
}

static UI_EVENTS: Lazy<broadcast::Sender<UiEventEnvelope>> = Lazy::new(|| {
    let (sender, _) = broadcast::channel(2048);
    sender
});

pub fn emit<T: Serialize>(name: &str, payload: T) {
    let payload = serde_json::to_value(payload).unwrap_or(Value::Null);
    let _ = UI_EVENTS.send(UiEventEnvelope {
        name: name.to_string(),
        payload,
    });
}

pub fn subscribe() -> broadcast::Receiver<UiEventEnvelope> {
    UI_EVENTS.subscribe()
}
