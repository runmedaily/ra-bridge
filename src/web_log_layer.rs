use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// A tracing layer that broadcasts formatted log lines to web UI clients.
pub struct WebLogLayer {
    tx: broadcast::Sender<String>,
}

impl WebLogLayer {
    pub fn new(tx: broadcast::Sender<String>) -> Self {
        Self { tx }
    }
}

struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: Vec::new(),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else {
            self.fields.push((field.name().to_string(), format!("{:?}", value)));
        }
    }
}

impl<S: Subscriber> Layer<S> for WebLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = metadata.level();
        let target = metadata.target();

        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        let mut line = format!("{} {} {}", level, target, visitor.message);
        for (k, v) in &visitor.fields {
            line.push_str(&format!(" {}={}", k, v));
        }

        // Best-effort send — if no subscribers, just drop
        let _ = self.tx.send(line);
    }
}
