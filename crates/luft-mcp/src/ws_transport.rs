//! WebSocket transport adapter for serving MCP over WS instead of stdio.
//!
//! Each WS text frame carries one JSON-RPC message (serialized as JSON text).
//! Binary frames are also accepted (parsed as JSON).

use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::Result;
use futures::{Sink, Stream, StreamExt};
use rmcp::{
    service::{RxJsonRpcMessage, RoleServer, TxJsonRpcMessage},
    ServiceExt,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    tungstenite::{Error as WsError, Message},
    WebSocketStream,
};

use crate::LuftMcpServer;

// ── Sink wrapper: TxJsonRpcMessage → WS text frame ─────────────────────

struct WsMcpSink<S>(
    futures::stream::SplitSink<WebSocketStream<S>, Message>,
);

impl<S> Sink<TxJsonRpcMessage<RoleServer>> for WsMcpSink<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Error = WsError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().0).poll_ready(cx)
    }

    fn start_send(
        self: Pin<&mut Self>,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> Result<(), Self::Error> {
        let text = serde_json::to_string(&item).expect("serialize json-rpc");
        Pin::new(&mut self.get_mut().0).start_send(Message::Text(text.into()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().0).poll_close(cx)
    }
}

// ── Stream wrapper: WS text/binary → RxJsonRpcMessage ──────────────────

struct WsMcpStream<S>(
    futures::stream::SplitStream<WebSocketStream<S>>,
);

impl<S> Stream for WsMcpStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Item = RxJsonRpcMessage<RoleServer>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use futures::StreamExt;
        match self.get_mut().0.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(Message::Text(text)))) => {
                let msg = serde_json::from_str(text.as_str()).ok();
                Poll::Ready(msg)
            }
            Poll::Ready(Some(Ok(Message::Binary(data)))) => {
                let msg = serde_json::from_slice(&data).ok();
                Poll::Ready(msg)
            }
            Poll::Ready(Some(Err(_))) | Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Ok(_))) => Poll::Pending,
            Poll::Pending => Poll::Pending,
        }
    }
}

// ── Public API ─────────────────────────────────────────────────────────

/// Serve an MCP server over an established WebSocket stream.
///
/// The caller is responsible for accepting the WS connection (e.g. via
/// `tokio_tungstenite::accept_async`).  This function drives the rmcp
/// service loop until the WS connection closes.
pub async fn serve_ws<S>(server: LuftMcpServer, ws: WebSocketStream<S>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = ws.split();
    let sink = WsMcpSink(sink);
    let stream = WsMcpStream(stream);

    let service = server.serve((sink, stream)).await?;
    service.waiting().await?;
    Ok(())
}
