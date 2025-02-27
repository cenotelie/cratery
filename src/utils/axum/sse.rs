/*******************************************************************************
 * Copyright (c) 2021 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! API for Server-Sent Events

use std::convert::Infallible;
use std::fmt::{Display, Formatter};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes, HttpBody};
use axum::http::{HeaderValue, Response, header};
use axum::response::IntoResponse;
use futures::Stream;
use http_body::Frame;
use serde::Serialize;

/// A Server-Sent Event
#[allow(clippy::struct_field_names, dead_code)]
pub struct Event<T> {
    /// The event type, to be serialized in the `event` field
    pub event_type: Option<String>,
    /// The event unique id, if any
    pub id: Option<String>,
    /// The payload
    pub data: T,
}

impl<T> Event<T> {
    /// Produces an event from a payload
    pub fn from_data(data: T) -> Event<T> {
        Self {
            event_type: None,
            id: None,
            data,
        }
    }
}

impl<T: Serialize> Display for Event<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(event_type) = self.event_type.as_deref() {
            writeln!(f, "event: {event_type}")?;
        }
        if let Some(id) = self.event_type.as_deref() {
            writeln!(f, "id: {id}")?;
        }
        let data = serde_json::to_string(&self.data).map_err(|_| std::fmt::Error)?;
        writeln!(f, "data: {data}\n")
    }
}

/// A stream of Server-Sent Events to be sent by axum
pub struct ServerSentEventStream<S>(S);

impl<S, T> ServerSentEventStream<S>
where
    S: Send + Stream<Item = Event<T>>,
    T: Serialize + Send + Unpin,
{
    /// Encapsulate the original stream
    pub fn new(stream: S) -> ServerSentEventStream<S> {
        ServerSentEventStream(stream)
    }
}

impl<S, T> HttpBody for ServerSentEventStream<S>
where
    S: Send + Stream<Item = Event<T>> + Unpin,
    T: Serialize + Send + Unpin,
{
    type Data = Bytes;

    type Error = Infallible;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let inner = Pin::new(&mut self.get_mut().0);
        match inner.poll_next(cx) {
            Poll::Ready(Some(event)) => {
                let data = event.to_string().into_bytes();
                Poll::Ready(Some(Ok(Frame::data(Bytes::from(data)))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S, T> IntoResponse for ServerSentEventStream<S>
where
    S: Send + Stream<Item = Event<T>> + Unpin + 'static,
    T: Serialize + Send + Unpin,
{
    fn into_response(self) -> axum::response::Response {
        let mut response = Response::new(Body::new(self));
        response
            .headers_mut()
            .append(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        response
            .headers_mut()
            .append(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}
