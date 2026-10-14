use std::{convert::Infallible, time::Duration};

use axum::{
    Extension,
    extract::State,
    http::{HeaderName, HeaderValue, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::{Stream, StreamExt, stream};
use tokio::sync::broadcast::{Receiver, error::RecvError};

use super::{ApiError, RequestId, RestState};
use crate::application::events::ArchiveChange;

const HEARTBEAT: Duration = Duration::from_secs(15);

fn resync() -> Event {
    Event::default().event("resync").data("{}")
}

fn change_event(change: &ArchiveChange) -> Event {
    Event::default()
        .event("archive_changed")
        .json_data(change)
        .unwrap_or_else(|_| resync())
}

/// Stream of an initial `resync`, then changes; `resync` again after receiver lag.
pub(in crate::interface::rest) fn event_stream(
    receiver: Receiver<ArchiveChange>,
    shutdown: tokio_util::sync::CancellationToken,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let changes = stream::unfold(receiver, |mut receiver| async move {
        let event = match receiver.recv().await {
            Ok(change) => change_event(&change),
            Err(RecvError::Lagged(_)) => resync(),
            Err(RecvError::Closed) => return None,
        };
        Some((Ok(event), receiver))
    });
    stream::once(async { Ok(resync()) })
        .chain(changes)
        .take_until(shutdown.cancelled_owned())
}

#[utoipa::path(
    get,
    path = "/api/v1/events",
    responses(
        (status = 200, description = "Server-sent events: `archive_changed` with JSON `{chat_ids:number[],global:boolean}` and `resync` with `{}` (sent on connect and after lag). Invalidation hints only; refetch via the REST API.", content_type = "text/event-stream", body = String),
        (status = 503, body = super::ErrorEnvelope, description = "No in-process collector (query-only mode)")
    )
)]
pub(in crate::interface::rest) async fn events(
    State(state): State<RestState>,
    Extension(id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let events = state
        .events
        .ok_or_else(|| ApiError::events_unavailable(id.0.clone()))?;
    // Subscribe before the initial resync so no commit falls between them.
    let stream = event_stream(events.subscribe(), events.shutdown_token());
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(HEARTBEAT))
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::events::ArchiveEvents;

    fn text(event: Result<Event, Infallible>) -> String {
        format!("{:?}", event.unwrap())
    }

    #[tokio::test]
    async fn lagged_receiver_gets_resync_then_ends_on_shutdown() {
        let events = ArchiveEvents::new(1);
        let receiver = events.subscribe();
        for id in 0..3 {
            events.publish(ArchiveChange {
                chat_ids: vec![id],
                global: false,
            });
        }
        let mut stream = Box::pin(event_stream(receiver, events.shutdown_token()));
        assert!(text(stream.next().await.unwrap()).contains("resync"));
        assert!(text(stream.next().await.unwrap()).contains("resync"));
        // Receiver then yields the newest retained change.
        assert!(text(stream.next().await.unwrap()).contains("archive_changed"));
        events.close();
        assert!(stream.next().await.is_none());
    }
}
