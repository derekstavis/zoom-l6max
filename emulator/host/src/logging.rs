//! Shared structured diagnostics for l6test, adapters and the host.
use tracing_subscriber::{EnvFilter, fmt};

pub const DEFAULT_FILTER: &str = "warn,l6test=info,smoke_core=info,l6max_gui=info";

#[derive(Clone, Copy)]
pub enum Format {
    Json,
    Text,
}

/// Install one subscriber at the executable boundary. Libraries only emit events.
/// RUST_LOG supports normal target/level filters; L6_LOG_FORMAT is json or text.
pub fn init(default: Format) -> Result<(), String> {
    let format = std::env::var("L6_LOG_FORMAT").unwrap_or_else(|_| match default {
        Format::Json => "json".into(),
        Format::Text => "text".into(),
    });
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_FILTER.into());
    let filter = EnvFilter::try_new(filter).map_err(|e| format!("invalid RUST_LOG: {e}"))?;
    let subscriber = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false);
    match format.as_str() {
        "json" => subscriber
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .try_init(),
        "text" => subscriber.compact().try_init(),
        _ => return Err("L6_LOG_FORMAT must be json or text".into()),
    }
    .map_err(|e| format!("could not install tracing subscriber: {e}"))
}

/// Context passed explicitly by the test supervisor, inherited by adapter events.
pub fn context(default_case: &str) -> tracing::Span {
    let case = std::env::var("SMOKE_CASE").unwrap_or_else(|_| default_case.into());
    let model = std::env::var("SMOKE_MODEL").ok();
    let backend = std::env::var("SMOKE_BACKEND").ok();
    context_fields(&case, model.as_deref(), backend.as_deref())
}

fn context_fields(case: &str, model: Option<&str>, backend: Option<&str>) -> tracing::Span {
    tracing::info_span!(target:"l6test", "l6test_context",case,model,backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn json_preserves_numeric_fields_span_context_and_level_filtering() {
        let buffer = Buffer(Arc::new(Mutex::new(vec![])));
        let writer = buffer.clone();
        let subscriber = fmt()
            .with_env_filter(EnvFilter::new(DEFAULT_FILTER))
            .with_writer(move || writer.clone())
            .json()
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let span = context_fields("panel", Some("l6"), Some("headless"));
            let _entered = span.enter();
            tracing::debug!(target:"smoke_core", "filtered event");
            tracing::info!(target:"smoke_core", step = 7_u64, guest_ms = 1234_u64, "step started");
        });
        let bytes = buffer.0.lock().unwrap();
        let lines = String::from_utf8(bytes.clone()).unwrap();
        let records: Vec<serde_json::Value> = lines
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["level"], "INFO");
        assert_eq!(records[0]["fields"]["step"], 7);
        assert_eq!(records[0]["span"]["model"], "l6");
    }
}
