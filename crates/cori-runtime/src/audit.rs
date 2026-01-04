pub use cori_core::{AuditEvent, AuditEventType};

/// MVP: trait boundary. Later can implement DB/Kafka/outbox.
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

/// Simple stdout sink for now.
pub struct StdoutAuditSink;

impl AuditSink for StdoutAuditSink {
    fn record(&self, event: AuditEvent) {
        println!(
            "[AUDIT] intent={} seq={:?} type={:?} step={} action={} allowed={} decision={} outcome={}",
            event.intent_id,
            event.sequence,
            event.event_type,
            event.step_id,
            event.action,
            event.allowed,
            serde_json::Value::Object(event.decision.clone()),
            serde_json::Value::Object(event.outcome.clone())
        );
    }
}
