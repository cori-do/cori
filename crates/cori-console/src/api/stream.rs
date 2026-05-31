//! `GET /api/runs/:run_id/stream` — SSE event stream for a
//! Console-triggered run.
//!
//! A subscriber gets the buffered events first (so a client that
//! connects after the run started still sees `plan` + earlier steps),
//! then live events as they fire. The stream closes after a terminal
//! event (`completed` / `failed`) so the SPA can tear down the
//! `EventSource`. Late subscribers (post-completion) see all buffered
//! events including the terminal one, and the same scan logic ends
//! the stream.

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures::Stream;
use futures::stream::{self, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use crate::{error::ApiError, runs::RunEvent, state::AppState};

pub async fn handler(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let runs = state.runs.read().await;
    let channel = runs
        .get(&run_id)
        .ok_or_else(|| ApiError::NotFound(format!("run `{run_id}` not active")))?
        .clone();
    drop(runs);

    let buffer = channel.snapshot();
    let rx = channel.tx.subscribe();

    let buffered = stream::iter(buffer.into_iter());
    let live = BroadcastStream::new(rx).filter_map(|res| async move { res.ok() });

    // Close the stream after the run's terminal event. Without this
    // the SSE connection stays open forever, since the broadcast
    // channel remains alive in `state.runs`.
    let combined = buffered.chain(live).scan(false, |done, ev| {
        if *done {
            return futures::future::ready(None);
        }
        if matches!(ev, RunEvent::Completed { .. } | RunEvent::Failed { .. }) {
            *done = true;
        }
        futures::future::ready(Some(ev))
    });

    let events = combined.map(|ev| -> Result<Event, Infallible> {
        let name = event_name(&ev);
        Ok(Event::default()
            .event(name)
            .json_data(&ev)
            .unwrap_or_else(|_| Event::default().event("error")))
    });

    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

fn event_name(ev: &RunEvent) -> &'static str {
    match ev {
        RunEvent::Plan { .. } => "plan",
        RunEvent::StepStart { .. } => "step_start",
        RunEvent::StepFinish { .. } => "step_finish",
        RunEvent::Completed { .. } => "completed",
        RunEvent::Failed { .. } => "failed",
    }
}
