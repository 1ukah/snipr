//! Servidor MCP local (Streamable HTTP, respostas JSON).
//!
//! Cada agente ganha um endpoint `/mcp/{agente}` ligado à própria aba. O
//! Claude Code (ou qualquer cliente MCP) conecta aqui para usar as
//! ferramentas do navegador. Só escuta em 127.0.0.1 e exige token.

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use parking_lot::RwLock;
use serde_json::{json, Value};

use crate::tools::ToolBox;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub struct McpServer {
    pub port: u16,
    token: String,
    registry: Arc<RwLock<HashMap<String, Arc<ToolBox>>>>,
}

#[derive(Clone)]
struct AppState {
    token: Arc<String>,
    registry: Arc<RwLock<HashMap<String, Arc<ToolBox>>>>,
}

impl McpServer {
    pub async fn start() -> Result<Arc<Self>> {
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let registry: Arc<RwLock<HashMap<String, Arc<ToolBox>>>> = Default::default();
        let state = AppState { token: Arc::new(token.clone()), registry: registry.clone() };
        let app = Router::new()
            .route("/mcp/{agent}", post(handle).get(no_sse).delete(no_sse))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("servidor MCP parou: {e}");
            }
        });
        tracing::info!(port, "servidor MCP local ativo");
        Ok(Arc::new(Self { port, token, registry }))
    }

    pub fn register(&self, agent: &str, tools: Arc<ToolBox>) {
        self.registry.write().insert(agent.to_string(), tools);
    }

    pub fn unregister(&self, agent: &str) {
        self.registry.write().remove(agent);
    }

    pub fn url_for(&self, agent: &str) -> String {
        format!("http://127.0.0.1:{}/mcp/{}", self.port, agent)
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

async fn no_sse() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    // Bloqueia requisições vindas de páginas web (DNS rebinding / CSRF).
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        if !(origin.starts_with("http://127.0.0.1") || origin.starts_with("http://localhost")) {
            return false;
        }
    }
    let got = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    // Comparação em tempo constante.
    got.len() == token.len() && got.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

async fn handle(State(st): State<AppState>, Path(agent): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&headers, &st.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let tools = st.registry.read().get(&agent).cloned();
    let Some(tools) = tools else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(rpc_error(Value::Null, -32700, "JSON inválido"))).into_response();
    };
    match msg {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for m in batch {
                if let Some(r) = dispatch(&tools, m).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                Json(Value::Array(out)).into_response()
            }
        }
        m => match dispatch(&tools, m).await {
            Some(r) => Json(r).into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    //! Ponta a ponta sem modelo de IA: fala com o servidor MCP exatamente como
    //! o Claude Code faria. Precisa de Chrome e internet:
    //!   cargo test -p snipr-core -- --ignored --nocapture
    use super::*;
    use crate::{
        sites::default_sites,
        tools::{HumanGates, Sink, ToolBoxInit},
        types::{ConditionFilter, SearchEvent},
    };
    use snipr_browser::{Browser, BrowserMode, LaunchOptions};

    async fn rpc(http: &reqwest::Client, url: &str, token: &str, body: Value) -> Value {
        let r = http.post(url).bearer_auth(token).json(&body).send().await.unwrap();
        assert!(r.status().is_success(), "status {}", r.status());
        r.json().await.unwrap()
    }

    #[tokio::test]
    #[ignore]
    async fn mcp_ponta_a_ponta() {
        let browser = Browser::launch(LaunchOptions {
            chrome_path: None,
            profile_dir: std::env::temp_dir().join("snipr-probe-profile"),
            mode: BrowserMode::Hidden,
            block_media: true,
            low_memory: true,
        })
        .await
        .unwrap();
        let page = browser.new_page().await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sites = default_sites();
        let sink = Arc::new(Sink { job: "t".into(), events: tx, listings: Default::default(), filter: ConditionFilter::Any, max_price: None });
        let tools = Arc::new(ToolBox::new(ToolBoxInit {
            agent: "t-0".into(),
            page: page.clone(),
            sites: sites.clone(),
            all_sites: sites,
            sessions: Default::default(),
            on_session: Arc::new(|_, _| {}),
            gates: Arc::new(HumanGates::default()),
            sink: sink.clone(),
        }));
        let server = McpServer::start().await.unwrap();
        server.register("t-0", tools.clone());
        let url = server.url_for("t-0");
        let http = reqwest::Client::new();

        // Sem token: recusado.
        let r = http.post(&url).json(&json!({"jsonrpc":"2.0","id":0,"method":"ping"})).send().await.unwrap();
        assert_eq!(r.status(), 401);

        let init = rpc(&http, &url, server.token(), json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})).await;
        assert_eq!(init["result"]["serverInfo"]["name"], "snipr");
        let list = rpc(&http, &url, server.token(), json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
        println!("ferramentas: {}", list["result"]["tools"].as_array().unwrap().len());

        let call = |id: i64, name: &str, args: Value| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}});
        let r = rpc(&http, &url, server.token(), call(3, "search_site", json!({"site_id":"olx","query":"cadeira dt3 n10"}))).await;
        println!("search_site → {}", r["result"]["content"][0]["text"]);
        let r = rpc(&http, &url, server.token(), call(4, "extract_listings", json!({}))).await;
        let text = r["result"]["content"][0]["text"].as_str().unwrap().to_string();
        println!("extract_listings →\n{}", text.lines().take(5).collect::<Vec<_>>().join("\n"));
        assert!(text.contains("R$"), "nenhum preço extraído");

        let r = rpc(&http, &url, server.token(), call(5, "report_listings", json!({"site_id":"olx","items":[
            {"id":1,"condition":"used"},
            {"title":"Peça sem preço","price":0,"condition":"used","url":"https://olx.com.br/y"}
        ]}))).await;
        println!("report_listings → {}", r["result"]["content"][0]["text"]);
        assert_eq!(sink.listings.lock().len(), 1);

        let mut n = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, SearchEvent::Listing { .. }) {
                n += 1;
            }
        }
        assert_eq!(n, 1);
        page.close().await;
        browser.close().await;
    }
}

/// Responde a uma mensagem JSON-RPC. Notificações (sem id) não têm resposta.
async fn dispatch(tools: &ToolBox, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg["method"].as_str().unwrap_or_default();
    let params = &msg["params"];
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "snipr", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Ferramentas de navegador do Snipr para pesquisar preços."
        }),
        "ping" => json!({}),
        "tools/list" => {
            let list: Vec<Value> = ToolBox::specs()
                .into_iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.schema }))
                .collect();
            json!({ "tools": list })
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let (text, is_error) = tools.call(name, &args).await;
            json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
        }
        "resources/list" => json!({ "resources": [] }),
        "prompts/list" => json!({ "prompts": [] }),
        _ => return Some(rpc_error(id, -32601, "método não suportado")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}
