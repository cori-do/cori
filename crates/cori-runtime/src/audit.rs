#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub intent_id: String,
    pub step_id: String,
    pub action: String,
    pub allowed: bool,
    pub decision: serde_json::Value,
    pub outcome: serde_json::Value,
}

/// MVP: trait boundary. Later can implement DB/Kafka/outbox.
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

/// Simple stdout sink for now.
pub struct StdoutAuditSink;

impl AuditSink for StdoutAuditSink {
    fn record(&self, event: AuditEvent) {
        println!(
            "[AUDIT] intent={} step={} action={} allowed={} decision={} outcome={}",
            event.intent_id,
            event.step_id,
            event.action,
            event.allowed,
            event.decision,
            event.outcome
        );
    }
}
