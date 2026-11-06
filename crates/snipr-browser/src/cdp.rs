//! Cliente Chrome DevTools Protocol mínimo.
//!
//! Uma única conexão WebSocket com o browser; cada aba é uma sessão "flatten"
//! (`sessionId`) multiplexada nessa conexão. Não usamos `Runtime.enable` em
//! nenhum momento — é o principal sinal de automação que sites anti-bot procuram.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    #[error("conexão com o navegador foi fechada")]
    Closed,
    #[error("tempo esgotado aguardando {0}")]
    Timeout(String),
    #[error("{method}: {message}")]
    Protocol { method: String, message: String },
    #[error("websocket: {0}")]
    Ws(String),
}

#[derive(Clone, Debug)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

type Reply = oneshot::Sender<Result<Value, CdpError>>;
type Pending = Arc<Mutex<HashMap<u64, (String, Reply)>>>;

pub struct Cdp {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    events: broadcast::Sender<CdpEvent>,
    closed: Arc<AtomicBool>,
}

impl Cdp {
    pub async fn connect(ws_url: &str) -> Result<Arc<Self>, CdpError> {
        let (ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| CdpError::Ws(e.to_string()))?;
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Default::default();
        let (ev_tx, _) = broadcast::channel(1024);
        let closed = Arc::new(AtomicBool::new(false));

        tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                if sink.send(Message::Text(m)).await.is_err() {
                    break;
                }
            }
        });

        let pending_r = pending.clone();
        let ev_r = ev_tx.clone();
        let closed_r = closed.clone();
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).into_owned(),
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    if let Some((method, tx)) = pending_r.lock().remove(&id) {
                        let r = match v.get("error") {
                            Some(err) => Err(CdpError::Protocol {
                                method,
                                message: err
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("erro desconhecido")
                                    .to_string(),
                            }),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = tx.send(r);
                    }
                } else if let Some(method) = v.get("method").and_then(Value::as_str) {
                    let _ = ev_r.send(CdpEvent {
                        method: method.to_string(),
                        params: v.get("params").cloned().unwrap_or(Value::Null),
                        session_id: v.get("sessionId").and_then(Value::as_str).map(String::from),
                    });
                }
            }
            closed_r.store(true, Ordering::SeqCst);
            for (_, (_, tx)) in pending_r.lock().drain() {
                let _ = tx.send(Err(CdpError::Closed));
            }
        });

        Ok(Arc::new(Self { out: out_tx, pending, next_id: AtomicU64::new(1), events: ev_tx, closed }))
    }

    pub async fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value, CdpError> {
        self.call_timeout(session, method, params, Duration::from_secs(45)).await
    }

    pub async fn call_timeout(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        dur: Duration,
    ) -> Result<Value, CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, (method.to_string(), tx));
        if self.out.send(msg.to_string()).is_err() {
            self.pending.lock().remove(&id);
            return Err(CdpError::Closed);
        }
        match tokio::time::timeout(dur, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(CdpError::Closed),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(CdpError::Timeout(method.to_string()))
            }
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}
