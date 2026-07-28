//! Run session handler: start/cancel workflows and stream events over WS.

use std::sync::Arc;

use anyhow::Result;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use luft::Luft;
use luft_core::contract::event::AgentEvent;
use luft_core::contract::event::RunStatus;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    #[serde(rename = "start")]
    Start {
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        resume_from_id: Option<String>,
        #[serde(default)]
        _args: serde_json::Value,
        #[serde(default)]
        _no_acp_raw: bool,
    },
    #[serde(rename = "cancel")]
    Cancel {},
}

/// Handle a `/run` WS connection.
pub async fn handle(
    ws: WebSocketStream<tokio::net::TcpStream>,
    luft: Arc<Luft>,
) -> Result<()> {
    let (mut ws_sink, mut ws_stream) = ws.split();

    let first = ws_stream.next().await;
    let client_msg: ClientMsg = match first {
        Some(Ok(Message::Text(text))) => match serde_json::from_str(text.as_str()) {
            Ok(m) => m,
            Err(e) => {
                send_msg(&mut ws_sink, ServerMsg::Error { message: &e.to_string() }).await;
                return Ok(());
            }
        },
        _ => return Ok(()),
    };

    let handle = match client_msg {
        ClientMsg::Start {
            script,
            resume_from_id,
            ..
        } => {
            let result = if let Some(id) = resume_from_id {
                luft.start_resume(&id).await
            } else if let Some(lua) = script {
                luft.start_script(&lua).await
            } else {
                send_msg(
                    &mut ws_sink,
                    ServerMsg::Error {
                        message: "must provide `script` or `resume_from_id`",
                    },
                )
                .await;
                return Ok(());
            };
            match result {
                Ok(h) => h,
                Err(e) => {
                    send_msg(&mut ws_sink, ServerMsg::Error { message: &e.to_string() }).await;
                    return Ok(());
                }
            }
        }
        ClientMsg::Cancel {} => return Ok(()),
    };

    let run_dir = handle.run_dir_name().to_string();
    let mut rx = handle.subscribe();

    send_msg(&mut ws_sink, ServerMsg::Started { run_id: &run_dir }).await;

    loop {
        tokio::select! {
            biased;

            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(ClientMsg::Cancel {}) = serde_json::from_str(text.as_str()) {
                            handle.cancel();
                        }
                    }
                    _ => break,
                }
            }

            recv = rx.recv() => {
                match recv {
                    Ok(event) => {
                        let is_done = matches!(event, AgentEvent::RunDone { .. });
                        let json = serde_json::to_value(&event).unwrap_or_default();
                        send_msg(&mut ws_sink, ServerMsg::Event { event: &json }).await;
                        if is_done {
                            let status = match &event {
                                AgentEvent::RunDone { status, .. } => run_status_str(*status),
                                _ => String::new(),
                            };
                            send_msg(&mut ws_sink, ServerMsg::Complete {
                                run_id: &run_dir,
                                status: &status,
                            }).await;
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        send_msg(&mut ws_sink, ServerMsg::Complete {
                            run_id: &run_dir,
                            status: "unknown",
                        }).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                }
            }
        }
    }

    // Keep handle alive until streaming finishes; drop detaches the tokio task
    drop(handle);
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ServerMsg<'a> {
    #[serde(rename = "started")]
    Started { run_id: &'a str },
    #[serde(rename = "event")]
    Event { event: &'a serde_json::Value },
    #[serde(rename = "complete")]
    Complete { run_id: &'a str, status: &'a str },
    #[serde(rename = "error")]
    Error { message: &'a str },
}

async fn send_msg(
    sink: &mut futures::stream::SplitSink<WebSocketStream<tokio::net::TcpStream>, Message>,
    msg: ServerMsg<'_>,
) {
    if let Ok(text) = serde_json::to_string(&msg) {
        let _ = sink.send(Message::Text(text.into())).await;
    }
}

fn run_status_str(s: RunStatus) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "unknown".to_string())
}
