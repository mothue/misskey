//! Pass-through observations on the original socket and TLS future. This module
//! never reads independently, buffers payloads, or changes protocol packets.

use crate::transport::AttemptObservation;
use crate::{IoDirection, SslReply};
use std::{
    fmt,
    future::Future,
    io::{self, IoSlice},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::{
    config::{SslMode, SslNegotiation},
    tls::TlsConnect,
};

const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 4, 210, 22, 47];

struct RequestObservation {
    written: usize,
    awaiting_reply: bool,
}

pub(crate) struct ObservedStream<S> {
    inner: S,
    observation: AttemptObservation,
    request: Option<RequestObservation>,
}
impl<S> ObservedStream<S> {
    #[cfg(test)]
    pub(crate) fn get_ref_for_test(&self) -> &S {
        &self.inner
    }

    pub(crate) fn new(
        inner: S,
        observation: AttemptObservation,
        mode: SslMode,
        negotiation: SslNegotiation,
    ) -> Self {
        let request = (mode != SslMode::Disable && negotiation == SslNegotiation::Postgres)
            .then_some(RequestObservation {
                written: 0,
                awaiting_reply: false,
            });
        Self {
            inner,
            observation,
            request,
        }
    }

    fn written(&mut self, bytes: impl Iterator<Item = u8>, count: usize, requested: bool) {
        self.observation.written(count, requested);
        let Some(request) = self.request.as_mut() else {
            return;
        };
        // Inspect only the actual transferred prefix of the known eight-byte
        // SSLRequest. A mismatch disables inspection without retaining bytes.
        if request.awaiting_reply && count != 0 {
            self.request = None;
            return;
        }
        for byte in bytes.take(count) {
            if SSL_REQUEST.get(request.written) != Some(&byte) {
                self.request = None;
                return;
            }
            request.written += 1;
            self.observation.ssl_request_written();
        }
        request.awaiting_reply = request.written == SSL_REQUEST.len();
    }

    fn received(&mut self, bytes: &[u8]) {
        if !bytes.is_empty()
            && self
                .request
                .as_ref()
                .is_some_and(|request| !request.awaiting_reply)
        {
            // Unexpected IO ordering does not establish an SSL reply.
            self.request = None;
            return;
        }
        if self
            .request
            .as_ref()
            .is_some_and(|request| request.awaiting_reply)
            && let Some(reply) = bytes.first()
        {
            self.observation.ssl_reply(match reply {
                b'S' => SslReply::Accepted,
                b'N' => SslReply::Declined,
                b'E' => SslReply::ServerError,
                other => SslReply::Other(*other),
            });
            self.request = None;
        }
    }
}
impl<S> fmt::Debug for ObservedStream<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ObservedStream")
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for ObservedStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let requested = buf.remaining() != 0;
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        match &result {
            Poll::Ready(Ok(())) => {
                let received = &buf.filled()[before..];
                this.observation.read(received.len(), requested);
                this.received(received);
            }
            Poll::Ready(Err(error)) => this.observation.io_error(IoDirection::Read, error),
            Poll::Pending => {}
        }
        result
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for ObservedStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(cx, buf);
        match &result {
            Poll::Ready(Ok(count)) => this.written(buf.iter().copied(), *count, !buf.is_empty()),
            Poll::Ready(Err(error)) => this.observation.io_error(IoDirection::Write, error),
            Poll::Pending => {}
        }
        result
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
        match &result {
            Poll::Ready(Ok(count)) => this.written(
                bufs.iter().flat_map(|buf| buf.iter().copied()),
                *count,
                bufs.iter().any(|buf| !buf.is_empty()),
            ),
            Poll::Ready(Err(error)) => this.observation.io_error(IoDirection::Write, error),
            Poll::Pending => {}
        }
        result
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_flush(cx);
        if let Poll::Ready(Err(error)) = &result {
            this.observation.io_error(IoDirection::Flush, error);
        }
        result
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_shutdown(cx);
        if let Poll::Ready(Err(error)) = &result {
            this.observation.io_error(IoDirection::Shutdown, error);
        }
        result
    }
}

pub(crate) struct ObservedTls<T> {
    inner: T,
    observation: AttemptObservation,
}
impl<T> ObservedTls<T> {
    pub(crate) fn new(inner: T, observation: AttemptObservation) -> Self {
        Self { inner, observation }
    }
}
impl<S, T: TlsConnect<S>> TlsConnect<S> for ObservedTls<T> {
    // The existing postgres-openssl stream and channel binding are unchanged.
    type Stream = T::Stream;
    type Error = T::Error;
    type Future = ObservedTlsFuture<T::Future>;

    fn connect(self, stream: S) -> Self::Future {
        ObservedTlsFuture {
            inner: Box::pin(self.inner.connect(stream)),
            observation: self.observation,
            started: false,
        }
    }
}

pub(crate) struct ObservedTlsFuture<F> {
    inner: Pin<Box<F>>,
    observation: AttemptObservation,
    started: bool,
}
impl<F, S, E> Future for ObservedTlsFuture<F>
where
    F: Future<Output = Result<S, E>>,
{
    type Output = Result<S, E>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if !this.started {
            this.started = true;
            this.observation.tls_connect_started();
        }
        let result = this.inner.as_mut().poll(cx);
        if let Poll::Ready(result) = &result {
            this.observation.tls_connect_finished(result.is_ok());
        }
        result
    }
}
