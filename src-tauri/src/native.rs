//! HTTP client for a running DSH instance's **native plugin management**
//! service (`POST /api/pluginManager/<method>`) — the one seam every launcher
//! plugin operation hangs on (ADR-0001: the running DSH owns plugin state).
//!
//! The network boundary sits behind [`NativeTransport`], so a test drives the
//! page states through a fake instance: it asserts on the request that *would*
//! be sent (envelope fields, endpoint, cookie) instead of needing a live DSH.
//! T2–T4 append write methods to [`NativeClient`]; the wire shapes and the two
//! list methods are frozen here.
//!
//! The facts encoded below are empirically verified — see the probe notes
//! (`native-api-probe.md`); do not "fix" them from memory.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::pin::Pin;
use std::sync::Arc;

/// Methods are addressed under the fixed `pluginManager/` envelope prefix;
/// the service rejects any `method` without it (`gateway/bad-request`).
const METHOD_PREFIX: &str = "pluginManager";

/// Endpoint prefix on the instance's HTTP surface.
const ENDPOINT_PREFIX: &str = "/api/pluginManager";

/// A boxed future, so [`NativeTransport`] stays object-safe (`Arc<dyn …>`)
/// without pulling in an async-trait dependency.
pub type NativeFuture<'a, T> =
    Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// A native **plugin entry** (the service's `PluginInfo`), narrowed to the
/// fields the launcher renders. `patchId` and `readOnlyReason` are mutually
/// exclusive in the native union; each is stored as its own `Option`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePlugin {
    /// Addressable node id in the composition tree; the UI row key and the
    /// `id` argument of `setPluginEnabled`.
    pub entry_id: String,
    /// The module specifier the loader imports, e.g.
    /// `@deepseek-ai/dsh-plugin-manager`.
    pub module_name: String,
    /// Localized display metadata; absent for many entries.
    #[serde(default)]
    pub meta: Option<NativeMeta>,
    /// Loader-level effective enable state.
    #[serde(default)]
    pub enabled: bool,
    /// Runtime phase: pending / loading / active / failed / unloading.
    #[serde(default)]
    pub fiber_phase: Option<String>,
    /// Present = manageable.
    #[serde(default)]
    pub patch_id: Option<String>,
    /// Present = not manageable (`management-required` | `unaddressable`).
    #[serde(default)]
    pub read_only_reason: Option<String>,
}

/// Localized display metadata carried on a plugin entry or a bundle.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NativeMeta {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A native **bundle** (a patch layer), narrowed to the fields the launcher
/// renders.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBundle {
    /// The `name` argument of `setBundleEnabled` / `removeBundle`.
    pub name: String,
    #[serde(default)]
    pub official: bool,
    /// `installation` | `profile` | `missing`.
    #[serde(default)]
    pub availability: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub meta: Option<NativeMeta>,
    #[serde(default)]
    pub description: Option<String>,
    /// Selected by the profile manifest.
    #[serde(default)]
    pub enabled: bool,
    /// Declared as a dependency by the profile.
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub source: Option<String>,
    /// Shipped with the install, offered as a user toggle.
    #[serde(default)]
    pub optional: bool,
    /// `false` = can only be disabled, never removed.
    #[serde(default)]
    pub removable: bool,
    #[serde(default)]
    pub read_only_reason: Option<String>,
    #[serde(default)]
    pub error: Option<NativeBundleError>,
}

/// A bundle-level failure as reported by the service.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBundleError {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub diagnostic: Option<serde_json::Value>,
}

/// One unary request the service strictly validates: **every field is
/// required**; extra or missing fields are rejected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "rpcId")]
    pub rpc_id: String,
    pub method: String,
    pub payload: NativePayload,
}

/// The `payload` wrapper of a request envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativePayload {
    pub args: serde_json::Value,
}

impl NativeEnvelope {
    /// Builds the `client-request` envelope for one method. `method` is the
    /// bare name (`listPlugins`); the `pluginManager/` prefix is added here
    /// because the service requires it in the envelope.
    pub fn request(method: &str, args: serde_json::Value) -> Self {
        Self {
            kind: "client-request".to_string(),
            rpc_id: uuid::Uuid::new_v4().to_string(),
            method: format!("{METHOD_PREFIX}/{method}"),
            payload: NativePayload { args },
        }
    }
}

/// A fully-formed request handed to the transport. A fake asserts on these
/// fields directly — no HTTP machinery in the test.
#[derive(Clone, Debug)]
pub struct NativeRequest {
    /// Full endpoint URL, e.g. `http://127.0.0.1:34567/api/pluginManager/listPlugins`.
    pub url: String,
    /// The `Cookie` header value carrying the minted session.
    pub cookie: String,
    /// The validated request body.
    pub envelope: NativeEnvelope,
}

/// The raw HTTP response, interpreted by [`NativeClient`].
#[derive(Clone, Debug)]
pub struct NativeResponse {
    pub status: u16,
    pub body: String,
}

/// The network boundary, injectable so tests substitute a fake instance.
/// `connect` mints the session cookie through [`mint`](Self::mint); `send`
/// performs one unary RPC. Both default to real HTTP via
/// [`ReqwestTransport`].
pub trait NativeTransport: Send + Sync {
    /// Exchanges a running-table URL's one-time token for the session cookie
    /// (`GET /?token=<token>`, `redirect: manual` — the `Set-Cookie` lives on
    /// the 303, so following it would fail). Returns the full `Cookie` header
    /// value to send on every RPC.
    ///
    /// The cookie is bound to `host:port`, so it is minted per instance per
    /// start and is never reused across ports.
    fn mint<'a>(&'a self, url: &'a str) -> NativeFuture<'a, Result<String, String>>;

    /// Sends one unary RPC and returns the raw HTTP response.
    fn send(
        &self,
        req: NativeRequest,
    ) -> NativeFuture<'_, Result<NativeResponse, String>>;
}

/// The real transport: `reqwest` over `rustls`, deliberately **not** proxied.
///
/// Instance traffic is always local (`127.0.0.1` / the bound host); a
/// user-configured proxy would swallow a localhost request — and
/// `proxy_enabled` defaults on, so routing through [`crate::proxy::apply`]
/// would break every instance call for proxy users. The `Host` header is left
/// to `reqwest` (it derives the correct one) and no `Origin` is sent (a wrong
/// `Origin` is a 403).
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("dsh-launcher")
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
        Ok(Self { client })
    }
}

impl NativeTransport for ReqwestTransport {
    fn mint<'a>(&'a self, url: &'a str) -> NativeFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let parsed =
                reqwest::Url::parse(url.trim()).map_err(|e| format!("实例 URL 无效: {e}"))?;
            let authority = authority_of(&parsed)?;
            let expected = cookie_name(&authority);
            let resp = self
                .client
                .get(url.trim())
                .send()
                .await
                .map_err(|e| format!("连接实例失败: {e}"))?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err("实例启动 token 无效或已过期，请重启实例".to_string());
            }
            let raw = resp
                .headers()
                .get(reqwest::header::SET_COOKIE)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| format!("实例未返回会话 Cookie（HTTP {status}）"))?;
            let pair = raw.split(';').next().unwrap_or("").trim().to_string();
            if !pair.starts_with(&format!("{expected}=")) {
                return Err("实例返回的会话 Cookie 与实例端口不匹配".to_string());
            }
            Ok(pair)
        })
    }

    fn send(
        &self,
        req: NativeRequest,
    ) -> NativeFuture<'_, Result<NativeResponse, String>> {
        Box::pin(async move {
            let body = serde_json::to_string(&req.envelope)
                .map_err(|e| format!("序列化请求失败: {e}"))?;
            let resp = self
                .client
                .post(&req.url)
                .headers(rpc_headers(&req.cookie))
                .body(body)
                .send()
                .await
                .map_err(|e| format!("连接实例失败: {e}"))?;
            let status = resp.status().as_u16();
            let body = resp
                .text()
                .await
                .map_err(|e| format!("读取实例响应失败: {e}"))?;
            Ok(NativeResponse { status, body })
        })
    }
}

/// Headers every RPC carries: the session cookie and a JSON content type.
/// No `Origin` (a wrong one is a 403) and no manual `Host` (reqwest derives
/// the correct one).
pub(crate) fn rpc_headers(cookie: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    if let Ok(value) = reqwest::header::HeaderValue::from_str(cookie) {
        headers.insert(reqwest::header::COOKIE, value);
    }
    headers
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// A connected instance: base URL, minted session cookie, and the transport.
pub struct NativeClient {
    base_url: String,
    cookie: String,
    transport: Arc<dyn NativeTransport>,
}

impl NativeClient {
    /// Connects to a running instance whose URL carries the one-time token
    /// (the `RunningInstance.url` value). The launcher reads credentials only
    /// from here — never `<DSH_HOME>/.credentials.yaml`.
    pub async fn connect(url: &str) -> Result<Self, String> {
        Self::connect_with(Arc::new(ReqwestTransport::new()?), url).await
    }

    /// The injectable form: tests pass a fake [`NativeTransport`].
    pub async fn connect_with(
        transport: Arc<dyn NativeTransport>,
        url: &str,
    ) -> Result<Self, String> {
        let base_url = base_url_of(url)?;
        // The token is the only credential source; a URL without one (an
        // adopted instance's bare URL) cannot be connected and must be
        // refused here rather than falling back to a credentials file.
        token_of(url)?;
        let cookie = transport.mint(url).await?;
        Ok(Self {
            base_url,
            cookie,
            transport,
        })
    }

    /// Sends one unary RPC and returns the unwrapped `result.value`.
    ///
    /// `method` is the bare name (`listPlugins`); the `pluginManager/` prefix
    /// is added to the envelope. HTTP status and `result.ok` are **both**
    /// checked: the service answers HTTP 200 even when it refuses.
    pub async fn call(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let req = NativeRequest {
            url: format!("{}{ENDPOINT_PREFIX}/{method}", self.base_url),
            cookie: self.cookie.clone(),
            envelope: NativeEnvelope::request(method, args),
        };
        let resp = self.transport.send(req).await?;
        parse_value(resp)
    }

    /// Lists the instance's plugin entries — including `@deepseek-ai/*` core
    /// entries, which the launcher's former file reader filtered out.
    pub async fn list_plugins(&self) -> Result<Vec<NativePlugin>, String> {
        let value = self.call("listPlugins", serde_json::json!({})).await?;
        serde_json::from_value(value).map_err(|e| format!("解析插件列表失败: {e}"))
    }

    /// Lists the instance's bundles (patch layers).
    pub async fn list_bundles(&self) -> Result<Vec<NativeBundle>, String> {
        let value = self.call("listBundles", serde_json::json!({})).await?;
        serde_json::from_value(value).map_err(|e| format!("解析 Bundle 列表失败: {e}"))
    }
}

/// Interprets a raw response: transport-layer failures (bad status, `not
/// found`, `ok:false`) are errors. A refused *operation* still arrives as
/// HTTP 200 + `ok:true` with the refusal inside `value` — that is the
/// caller's business (see design §4b).
fn parse_value(resp: NativeResponse) -> Result<serde_json::Value, String> {
    match resp.status {
        404 => {
            return Err(format!(
                "实例不支持该操作（HTTP 404：{}）",
                resp.body.trim()
            ));
        }
        401 => return Err("实例会话已失效（HTTP 401），请重启实例".to_string()),
        403 => return Err("实例拒绝了请求（HTTP 403）".to_string()),
        s if !(200..300).contains(&s) => {
            return Err(format!("实例返回 HTTP {s}"));
        }
        _ => {}
    }
    let doc: serde_json::Value =
        serde_json::from_str(&resp.body).map_err(|e| format!("实例响应不是 JSON: {e}"))?;
    let result = doc
        .get("result")
        .ok_or_else(|| "实例响应缺少 result".to_string())?;
    match result.get("ok").and_then(|v| v.as_bool()) {
        Some(true) => Ok(result.get("value").cloned().unwrap_or(serde_json::Value::Null)),
        Some(false) => {
            let code = result
                .pointer("/error/code")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let message = result
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("实例拒绝了请求");
            if code.is_empty() {
                Err(format!("实例拒绝了请求：{message}"))
            } else {
                Err(format!("实例拒绝了请求（{code}）：{message}"))
            }
        }
        None => Err("实例响应缺少 result.ok".to_string()),
    }
}

// ---------------------------------------------------------------------------
// URL / credential helpers
// ---------------------------------------------------------------------------

/// The instance origin from a running-table URL, dropping the query (which
/// carries the one-time token), e.g. `http://127.0.0.1:34567`.
fn base_url_of(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|e| format!("实例 URL 无效: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("实例 URL 协议不支持：{other}")),
    }
    let authority = authority_of(&parsed)?;
    Ok(format!("{}://{authority}", parsed.scheme()))
}

/// The `host:port` authority, which the session cookie is bound to.
fn authority_of(parsed: &reqwest::Url) -> Result<String, String> {
    let host = parsed
        .host_str()
        .ok_or_else(|| "实例 URL 缺少主机".to_string())?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| "实例 URL 缺少端口".to_string())?;
    Ok(format!("{host}:{port}"))
}

/// The token from the running-table URL's `?token=` query. This is the only
/// credential source; `<DSH_HOME>/.credentials.yaml` is never read.
fn token_of(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|e| format!("实例 URL 无效: {e}"))?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "实例 URL 缺少启动 token".to_string())
}

/// The auth cookie name the instance mints:
/// `dsh-auth-<base64url(sha256("<host>:<port>"))>` with no padding.
fn cookie_name(authority: &str) -> String {
    let digest = Sha256::digest(authority.as_bytes());
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    format!("dsh-auth-{encoded}")
}

// ---------------------------------------------------------------------------
// The view model handed to the Plugins page
// ---------------------------------------------------------------------------

/// Why the page is managing (or not): determines managed vs read-only UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginViewMode {
    /// Instance reachable and a session cookie was minted: operations allowed.
    Managed,
    /// No instance is running this profile.
    NotRunning,
    /// The instance was adopted from an external process (no token).
    Adopted,
    /// An instance is running but refused the connection / cookie.
    Unreachable,
}

/// A **plugin entry** (the native `PluginInfo`'s stable subset).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginItem {
    /// Row key; the `id` argument of `setPluginEnabled`.
    pub entry_id: String,
    pub module_name: String,
    /// `meta.title`; the UI falls back to `moduleName` when absent.
    pub title: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub fiber_phase: Option<String>,
    pub patch_id: Option<String>,
    /// `management-required` | `unaddressable`.
    pub read_only_reason: Option<String>,
}

/// A **Bundle（补丁层）** — the native `BundleInfo`'s stable subset.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleItem {
    /// The `name` argument of `setBundleEnabled` / `removeBundle`.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub enabled: bool,
    pub installed: bool,
    /// `false` = can only be disabled, never removed.
    pub removable: bool,
    pub optional: bool,
    /// `installation` | `profile` | `missing`.
    pub availability: Option<String>,
    pub read_only_reason: Option<String>,
    pub error_code: Option<String>,
}

/// One snapshot of the Plugins page: `mode` says whether operations are
/// allowed, `reason` is the localized read-only explanation the UI shows
/// verbatim.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub mode: PluginViewMode,
    pub reason: Option<String>,
    pub plugins: Vec<PluginItem>,
    pub bundles: Vec<BundleItem>,
}

impl PluginView {
    /// A managed snapshot, projected from the instance's two lists.
    pub fn managed(plugins: Vec<NativePlugin>, bundles: Vec<NativeBundle>) -> Self {
        Self {
            mode: PluginViewMode::Managed,
            reason: None,
            plugins: plugins.into_iter().map(PluginItem::from).collect(),
            bundles: bundles.into_iter().map(BundleItem::from).collect(),
        }
    }

    /// A read-only snapshot with empty lists and the localized reason.
    pub fn readonly(mode: PluginViewMode, reason: impl Into<String>) -> Self {
        Self {
            mode,
            reason: Some(reason.into()),
            plugins: Vec::new(),
            bundles: Vec::new(),
        }
    }
}

impl From<NativePlugin> for PluginItem {
    fn from(p: NativePlugin) -> Self {
        let meta = p.meta.unwrap_or_default();
        Self {
            entry_id: p.entry_id,
            module_name: p.module_name,
            title: meta.title,
            description: meta.description,
            enabled: p.enabled,
            fiber_phase: p.fiber_phase,
            patch_id: p.patch_id,
            read_only_reason: p.read_only_reason,
        }
    }
}

impl From<NativeBundle> for BundleItem {
    fn from(b: NativeBundle) -> Self {
        let meta = b.meta.unwrap_or_default();
        Self {
            name: b.name,
            title: meta.title,
            description: meta.description.or(b.description),
            version: b.version,
            enabled: b.enabled,
            installed: b.installed,
            removable: b.removable,
            optional: b.optional,
            availability: b.availability,
            read_only_reason: b.read_only_reason,
            error_code: b.error.and_then(|e| e.code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A fake instance: records every request, replies from a queue.
    struct FakeTransport {
        cookie: String,
        minted: Mutex<Vec<String>>,
        sent: Mutex<Vec<NativeRequest>>,
        responses: Mutex<VecDeque<NativeResponse>>,
    }

    impl FakeTransport {
        fn new(cookie: &str, responses: Vec<NativeResponse>) -> Arc<Self> {
            Arc::new(Self {
                cookie: cookie.to_string(),
                minted: Mutex::new(Vec::new()),
                sent: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into()),
            })
        }

        fn last_sent(&self) -> NativeRequest {
            self.sent.lock().unwrap().last().cloned().unwrap()
        }
    }

    impl NativeTransport for FakeTransport {
        fn mint<'a>(&'a self, url: &'a str) -> NativeFuture<'a, Result<String, String>> {
            Box::pin(async move {
                self.minted.lock().unwrap().push(url.to_string());
                Ok(self.cookie.clone())
            })
        }

        fn send(
            &self,
            req: NativeRequest,
        ) -> NativeFuture<'_, Result<NativeResponse, String>> {
            Box::pin(async move {
                self.sent.lock().unwrap().push(req);
                Ok(self.responses.lock().unwrap().pop_front().unwrap())
            })
        }
    }

    const INSTANCE_URL: &str =
        "http://127.0.0.1:34567/?token=JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";
    const COOKIE: &str = "dsh-auth-6x_m4cbVo7y_dJvM1GEEE82y47XOJvfDgntZoIfis6E=v1.payload.sig";

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn ok_response(value: serde_json::Value) -> NativeResponse {
        NativeResponse {
            status: 200,
            body: serde_json::json!({
                "type": "server-response",
                "rpcId": "x",
                "result": { "ok": true, "value": value },
            })
            .to_string(),
        }
    }

    fn client_with(responses: Vec<NativeResponse>) -> (NativeClient, Arc<FakeTransport>) {
        let fake = FakeTransport::new(COOKIE, responses);
        let transport: Arc<dyn NativeTransport> = fake.clone();
        let client = block_on(NativeClient::connect_with(transport, INSTANCE_URL)).unwrap();
        (client, fake)
    }

    /// The one request the service strictly validates: envelope shape, the
    /// `pluginManager/` prefix, endpoint path, and the session cookie — with
    /// no `Origin` header possible.
    #[test]
    fn list_plugins_sends_the_verified_envelope() {
        let (client, fake) = client_with(vec![ok_response(serde_json::json!([]))]);
        block_on(client.list_plugins()).unwrap();

        let req = fake.last_sent();
        assert_eq!(
            req.url,
            "http://127.0.0.1:34567/api/pluginManager/listPlugins"
        );
        assert_eq!(req.cookie, COOKIE);
        assert_eq!(req.envelope.kind, "client-request");
        assert_eq!(req.envelope.method, "pluginManager/listPlugins");
        assert_eq!(req.envelope.payload.args, serde_json::json!({}));
        assert!(!req.envelope.rpc_id.is_empty(), "rpcId must be present");

        // Exactly the four wire fields, nothing extra that the service rejects.
        let body = serde_json::to_value(&req.envelope).unwrap();
        let keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 4, "body: {body}");
        assert!(body.get("type").is_some());
        assert!(body.get("rpcId").is_some());
        assert!(body.get("method").is_some());
        assert!(body.get("payload").is_some());
    }

    /// Minting uses the running-table URL itself (token included); the cookie
    /// name is derived from `host:port`, verified against the probe's value.
    #[test]
    fn mint_uses_the_instance_url_and_binds_cookie_to_host_port() {
        let (_, fake) = client_with(vec![]);
        assert_eq!(fake.minted.lock().unwrap().as_slice(), &[INSTANCE_URL]);
        assert_eq!(
            cookie_name("127.0.0.1:34567"),
            "dsh-auth-6x_m4cbVo7y_dJvM1GEEE82y47XOJvfDgntZoIfis6E"
        );
    }

    /// The list is exactly what the instance returns — the `@deepseek-ai/*`
    /// core entries the old file reader dropped must survive the parse.
    #[test]
    fn list_plugins_keeps_deepseek_ai_entries_and_optional_fields() {
        let payload = serde_json::json!([
            {
                "entryId": "include:plugin-manager",
                "moduleName": "@deepseek-ai/dsh-plugin-manager",
                "enabled": true,
                "fiberPhase": "active",
                "meta": { "title": "插件管理", "description": "内置" },
                "patchId": "p1"
            },
            {
                "entryId": "include:llm",
                "moduleName": "@deepseek-ai/dsh-llm",
                "enabled": false,
                "fiberPhase": null,
                "readOnlyReason": "management-required"
            }
        ]);
        let (client, _) = client_with(vec![ok_response(payload)]);
        let plugins = block_on(client.list_plugins()).unwrap();

        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].entry_id, "include:plugin-manager");
        assert_eq!(plugins[0].module_name, "@deepseek-ai/dsh-plugin-manager");
        assert_eq!(
            plugins[0].meta.as_ref().unwrap().title.as_deref(),
            Some("插件管理")
        );
        assert_eq!(plugins[0].patch_id.as_deref(), Some("p1"));
        assert!(plugins[0].read_only_reason.is_none());
        // Options may be absent entirely — the second entry has no `meta`.
        assert_eq!(plugins[1].entry_id, "include:llm");
        assert!(plugins[1].meta.is_none());
        assert_eq!(
            plugins[1].read_only_reason.as_deref(),
            Some("management-required")
        );
    }

    #[test]
    fn list_bundles_parses_native_bundle_shape() {
        let payload = serde_json::json!([
            {
                "name": "@deepseek-ai/dsh-base",
                "official": true,
                "availability": "installation",
                "version": "0.2.1-alpha.2",
                "enabled": true,
                "installed": true,
                "removable": false,
                "optional": false
            }
        ]);
        let (client, fake) = client_with(vec![ok_response(payload)]);
        let bundles = block_on(client.list_bundles()).unwrap();
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].name, "@deepseek-ai/dsh-base");
        assert!(!bundles[0].removable);
        assert_eq!(
            fake.last_sent().url,
            "http://127.0.0.1:34567/api/pluginManager/listBundles"
        );
    }

    /// Transport-layer failures are errors; an operation refusal (200 +
    /// `ok:false`) is too.
    #[test]
    fn transport_and_envelope_failures_surface_as_err() {
        let cases = vec![
            NativeResponse { status: 401, body: String::new() },
            NativeResponse { status: 403, body: String::new() },
            NativeResponse { status: 404, body: "not found".to_string() },
            NativeResponse {
                status: 200,
                body: serde_json::json!({
                    "type": "server-response",
                    "rpcId": "x",
                    "result": { "ok": false, "error": { "code": "gateway/arguments-invalid" } },
                })
                .to_string(),
            },
        ];
        for resp in cases {
            let status = resp.status;
            let (client, _) = client_with(vec![resp]);
            assert!(
                block_on(client.list_plugins()).is_err(),
                "status {status} must be an error"
            );
        }
    }

    #[test]
    fn token_and_base_url_helpers_reject_bad_input() {
        assert_eq!(token_of(INSTANCE_URL).unwrap(), "JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ");
        assert!(token_of("http://127.0.0.1:34567/").is_err());
        assert_eq!(base_url_of(INSTANCE_URL).unwrap(), "http://127.0.0.1:34567");
        assert!(base_url_of("ftp://127.0.0.1:21/").is_err());
        assert!(base_url_of("not a url").is_err());
    }

    /// A bare URL (an adopted instance) has no token and must be refused
    /// before any traffic — the credential never comes from a file.
    #[test]
    fn connect_refuses_a_url_without_a_token() {
        let fake = FakeTransport::new(COOKIE, vec![]);
        let transport: Arc<dyn NativeTransport> = fake.clone();
        let err = match block_on(NativeClient::connect_with(
            transport,
            "http://127.0.0.1:3089",
        )) {
            Ok(_) => panic!("a tokenless URL must be refused"),
            Err(e) => e,
        };
        assert!(err.contains("token"), "err: {err}");
        assert!(fake.minted.lock().unwrap().is_empty());
    }

    /// No `Origin` (a wrong one is 403) and no manual `Host`; the cookie and
    /// content type are the only headers we set.
    #[test]
    fn rpc_headers_carry_cookie_without_origin_or_host() {
        let headers = rpc_headers(COOKIE);
        assert_eq!(
            headers.get(reqwest::header::COOKIE).unwrap(),
            COOKIE
        );
        assert_eq!(
            headers.get(reqwest::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert!(headers.get(reqwest::header::ORIGIN).is_none());
        assert!(headers.get(reqwest::header::HOST).is_none());
    }

    /// The read-only snapshot carries the mode + localized reason and no rows,
    /// so T5 only has to add content and UI.
    #[test]
    fn readonly_view_is_empty_with_mode_and_reason() {
        let view = PluginView::readonly(PluginViewMode::NotRunning, "实例未运行");
        assert_eq!(view.mode, PluginViewMode::NotRunning);
        assert_eq!(view.reason.as_deref(), Some("实例未运行"));
        assert!(view.plugins.is_empty());
        assert!(view.bundles.is_empty());
    }

    /// Projection: `meta.title` becomes `title`, and a bundle's error code
    /// surfaces for the UI. Serialization is camelCase for the frontend.
    #[test]
    fn managed_view_projects_meta_and_bundle_error() {
        let plugins = serde_json::from_value(serde_json::json!([{
            "entryId": "include:llm",
            "moduleName": "@deepseek-ai/dsh-llm",
            "enabled": true,
            "meta": { "title": "LLM" }
        }]))
        .unwrap();
        let bundles = serde_json::from_value(serde_json::json!([{
            "name": "ds-bot",
            "enabled": false,
            "installed": true,
            "removable": true,
            "optional": false,
            "error": { "code": "bundle-in-use" }
        }]))
        .unwrap();
        let view = PluginView::managed(plugins, bundles);
        assert_eq!(view.mode, PluginViewMode::Managed);
        assert_eq!(view.plugins[0].title.as_deref(), Some("LLM"));
        assert_eq!(view.bundles[0].name, "ds-bot");
        assert_eq!(view.bundles[0].error_code.as_deref(), Some("bundle-in-use"));

        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["plugins"][0]["entryId"], "include:llm");
        assert_eq!(json["mode"], "managed");
    }
}
