use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use futures_core::Stream;

use crate::state::AppState;

pub async fn pair_status_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.pairing_status.subscribe();

    let stream = async_stream::stream! {
        // Send current status immediately
        let current = rx.borrow().clone();
        if let Ok(json) = serde_json::to_string(&current) {
            yield Ok(Event::default().data(json));
        }

        // Then stream changes
        loop {
            match rx.changed().await {
                Ok(()) => {
                    let status = rx.borrow().clone();
                    if let Ok(json) = serde_json::to_string(&status) {
                        yield Ok(Event::default().data(json));
                    }
                }
                Err(_) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

pub async fn savant_discovery_status_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.savant_discovery_status.subscribe();

    let stream = async_stream::stream! {
        // Send current status immediately
        let current = rx.borrow().clone();
        if let Ok(json) = serde_json::to_string(&current) {
            yield Ok(Event::default().data(json));
        }

        // Then stream changes
        loop {
            match rx.changed().await {
                Ok(()) => {
                    let status = rx.borrow().clone();
                    if let Ok(json) = serde_json::to_string(&status) {
                        yield Ok(Event::default().data(json));
                    }
                }
                Err(_) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

pub async fn log_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.log_tx.subscribe();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    yield Ok(Event::default().data(line));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    yield Ok(Event::default().data(format!("... skipped {} log lines ...", n)));
                }
                Err(_) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

pub async fn zone_events_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let zone_levels = state.zone_levels.clone();

    let stream = async_stream::stream! {
        let mut last_snapshot: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;

            let current = zone_levels.read().await.clone();
            let mut changes = Vec::new();

            for (id, level) in &current {
                if last_snapshot.get(id) != Some(level) {
                    changes.push(serde_json::json!({
                        "id": id,
                        "level": level,
                    }));
                }
            }

            if !changes.is_empty() {
                last_snapshot = current;
                if let Ok(json) = serde_json::to_string(&changes) {
                    yield Ok(Event::default().data(json));
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}
