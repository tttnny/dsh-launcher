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

/// One **row** of a bundle: the composition-tree node a bundle contributes.
///
/// The page manages patch layers, not rows, so this exists to *count* them —
/// what a layer brings in is a fact about the layer. Everything but the count
/// is optional: the service omits `entryId` for rows that are not
/// individually addressable, and a local build may ship rows without metadata.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBundleRow {
    #[serde(default)]
    pub entry_id: Option<String>,
    #[serde(default)]
    pub module_name: Option<String>,
    #[serde(default)]
    pub meta: Option<NativeMeta>,
}

/// A native text field: plain text, or the locale map the service uses for its
/// own localized copy (`{"en": …, "zh": …}`).
///
/// Both shapes occur in one payload — DSH localizes the copy it ships and
/// leaves the rest as plain strings — so a field that can be either must accept
/// both. Reading one of these as `String` is what makes a whole list
/// unparseable the moment a single layer carries a translated title.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeText {
    Plain(String),
    Localized(std::collections::BTreeMap<String, String>),
}

impl NativeText {
    /// The text for `locale` (`zh-CN` also matches a plain `zh` key), then the
    /// service's own `en` fallback, then whatever the map happens to hold.
    pub fn pick(&self, locale: &str) -> String {
        match self {
            Self::Plain(text) => text.clone(),
            Self::Localized(map) => {
                let base = locale.split(['-', '_']).next().unwrap_or(locale);
                [locale, base, "en"]
                    .iter()
                    .find_map(|key| map.get(*key))
                    .or_else(|| map.values().next())
                    .cloned()
                    .unwrap_or_default()
            }
        }
    }

    /// [`pick`](Self::pick), with an empty result reading as "no text" so a
    /// blank translation does not become a blank line in the list.
    pub fn pick_opt(&self, locale: &str) -> Option<String> {
        let text = self.pick(locale);
        (!text.trim().is_empty()).then_some(text)
    }
}

/// Localized display metadata carried on a bundle or one of its rows.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NativeMeta {
    #[serde(default)]
    pub title: Option<NativeText>,
    #[serde(default)]
    pub description: Option<NativeText>,
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
    pub description: Option<NativeText>,
    /// Selected by the profile manifest.
    #[serde(default)]
    pub enabled: bool,
    /// Declared as a dependency by the profile.
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub source: Option<String>,
    /// The rows this layer contributes to the composition tree.
    #[serde(default)]
    pub rows: Vec<NativeBundleRow>,
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

/// The native `ChangeResult` — the answer to a write method, narrowed to the
/// stable subset the launcher shows. **A refused operation still returns HTTP
/// 200 + `result.ok:true`**: the refusal arrives here as `changed:false` plus
/// an `error.code`, which is "this cannot be changed", not a transport error
/// (design §4b). Shared by T2/T3/T4 — there is exactly one definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeChangeResult {
    pub changed: bool,
    /// `applied` | `restart-required` | `overridden` | `failed` | `cancelled`.
    /// Carried through as a string: the set has five members, not three.
    pub application: String,
    /// `install` | `enable` | `remove`.
    pub stage: String,
    pub target: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub error: Option<NativeChangeError>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Why a write was refused (`ManagementError`): the `code` (`management-required`
/// | `unaddressable` | `not-removable` | …) plus an optional `diagnostic`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeChangeError {
    pub code: String,
    #[serde(default)]
    pub diagnostic: Option<NativeText>,
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

    /// Lists the instance's bundles (patch layers).
    pub async fn list_bundles(&self) -> Result<Vec<NativeBundle>, String> {
        let value = self.call("listBundles", serde_json::json!({})).await?;
        serde_json::from_value(value).map_err(|e| format!("解析 Bundle 列表失败: {e}"))
    }

    /// Enables/disables one whole **Bundle（补丁层）** by `name` (the native
    /// argument is `name`, not `id`). Re-enabling appends the bundle to the
    /// **end** of the profile's ordered bundle list, so its priority shifts.
    ///
    /// A refused operation (e.g. `removable:false` → `management-required`) is
    /// returned as a [`NativeChangeResult`], not an `Err`; only transport-layer
    /// failures and `ok:false` are errors.
    pub async fn set_bundle_enabled(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<NativeChangeResult, String> {
        let value = self
            .call("setBundleEnabled", serde_json::json!({ "name": name, "enabled": enabled }))
            .await?;
        serde_json::from_value(value).map_err(|e| format!("解析改动结果失败: {e}"))
    }

    /// Removes one whole **Bundle（补丁层）** by `name` (the native
    /// `removeBundle` argument is `name` only — no `enabled`). This is the
    /// native removal path the Web UI uses, so a plugin the Web UI can remove
    /// the launcher can too.
    ///
    /// A bundle that ships with DSH (`removable:false`) is **refused** as
    /// HTTP 200 + `result.ok:true` with `changed:false` +
    /// `error.code:"not-removable"` — returned as a [`NativeChangeResult`] so
    /// the page can show the reason, not raised as an `Err` (design §4b). Only
    /// transport-layer failures and `ok:false` are errors.
    pub async fn remove_bundle(&self, name: &str) -> Result<NativeChangeResult, String> {
        let value = self
            .call("removeBundle", serde_json::json!({ "name": name }))
            .await?;
        serde_json::from_value(value).map_err(|e| format!("解析卸载结果失败: {e}"))
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

/// A **Bundle（补丁层）** — the native `BundleInfo`'s stable subset.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleItem {
    /// The `name` argument of `setBundleEnabled` / `removeBundle`.
    pub name: String,
    /// How many composition-tree rows this layer contributes. The rows are not
    /// a control surface here; the count is what makes "one whole layer" — the
    /// unit this page manages — concrete.
    pub row_count: usize,
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
///
/// The bundles（补丁层）are the whole page: a bundle is the unit the profile
/// selects and DSH mounts, and it is what carries a version, an availability
/// and a removal rule. The rows inside it are DSH's own composition detail, not
/// a control surface the launcher should hand out one switch at a time.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub mode: PluginViewMode,
    /// Which instance answered. The page was opened from one instance's row,
    /// but (HOME, Profile) can be running in two of them at once: this is the
    /// one that actually resolved, so the page can name it instead of assuming.
    pub instance_id: Option<String>,
    pub reason: Option<String>,
    pub bundles: Vec<BundleItem>,
}

impl PluginView {
    /// A managed snapshot, projected from the instance's own bundle list.
    /// `instance_id` is the instance that answered (see [`Self::instance_id`]).
    pub fn managed(bundles: Vec<NativeBundle>, locale: &str, instance_id: &str) -> Self {
        Self {
            mode: PluginViewMode::Managed,
            instance_id: Some(instance_id.to_string()),
            reason: None,
            bundles: bundles
                .into_iter()
                .map(|b| BundleItem::from_native(b, locale))
                .collect(),
        }
    }

    /// A read-only snapshot: no instance is running that profile (or the one
    /// that is cannot be authenticated), so there is nothing to manage and the
    /// localization-ready `reason` says why. `bundles` is what the profile's
    /// own files say it holds — a best-effort projection that the page labels
    /// as such (design §4: it never writes, and it differs from the live list
    /// on purpose).
    pub fn readonly(
        mode: PluginViewMode,
        reason: impl Into<String>,
        bundles: Vec<BundleItem>,
    ) -> Self {
        Self {
            mode,
            instance_id: None,
            reason: Some(reason.into()),
            bundles,
        }
    }
}

impl BundleItem {
    /// One layer as the profile's own files declare it — the read-only
    /// projection used when no instance can answer. It carries only what a
    /// file says: that the profile selects this layer, that it installs that
    /// package, and (when the package is on disk) its version. Live state,
    /// availability, removal rules and row counts are the instance's business,
    /// so they stay empty here rather than being guessed.
    pub fn from_profile_files(
        name: String,
        selected: bool,
        installed: bool,
        version: Option<String>,
    ) -> Self {
        Self {
            name,
            row_count: 0,
            title: None,
            description: None,
            version,
            enabled: selected,
            installed,
            // We cannot see the installation's manifest from here, so we never
            // claim a layer is removable: the page is read-only anyway.
            removable: false,
            optional: false,
            availability: None,
            read_only_reason: None,
            error_code: None,
        }
    }

    /// Projects one native bundle; its own `description` wins over the one
    /// carried in `meta`, exactly as before localization existed.
    pub fn from_native(b: NativeBundle, locale: &str) -> Self {
        let meta = b.meta.unwrap_or_default();
        Self {
            name: b.name,
            row_count: b.rows.len(),
            title: meta.title.as_ref().and_then(|t| t.pick_opt(locale)),
            description: meta
                .description
                .as_ref()
                .or(b.description.as_ref())
                .and_then(|d| d.pick_opt(locale)),
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
    fn list_bundles_sends_the_verified_envelope() {
        let (client, fake) = client_with(vec![ok_response(serde_json::json!([]))]);
        block_on(client.list_bundles()).unwrap();

        let req = fake.last_sent();
        assert_eq!(
            req.url,
            "http://127.0.0.1:34567/api/pluginManager/listBundles"
        );
        assert_eq!(req.cookie, COOKIE);
        assert_eq!(req.envelope.kind, "client-request");
        assert_eq!(req.envelope.method, "pluginManager/listBundles");
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

    /// The service localizes its own copy: a bundle's `meta.title` is a plain
    /// string for most layers and a `{en, zh}` map for the ones it translated.
    /// Both shapes have to parse — reading the map as a string is what made one
    /// translated title fail the *whole* list, and with it the page.
    #[test]
    fn a_localized_title_does_not_fail_the_bundle_list() {
        let payload = serde_json::json!([
            {
                "name": "@deepseek-ai/dsh-base",
                "enabled": true,
                "installed": true,
                "removable": false,
                "optional": false,
                "meta": { "title": "@deepseek-ai/dsh-base", "description": "The core" },
                "rows": [{ "entryId": "include:llm" }, { "moduleName": "x" }]
            },
            {
                "name": "@deepseek-ai/dsh-schedule",
                "enabled": true,
                "installed": true,
                "removable": true,
                "optional": false,
                "meta": {
                    "title": { "en": "Task scheduling", "zh": "任务调度" },
                    "description": { "en": "Stores tasks", "zh": "保存任务" }
                },
                "rows": [{ "entryId": "include:schedule" }]
            },
            {
                "name": "bare",
                "enabled": false,
                "installed": false,
                "removable": false,
                "optional": false
            }
        ]);
        let (client, _) = client_with(vec![ok_response(payload)]);
        let bundles = block_on(client.list_bundles())
            .expect("a translated title must not fail the list");

        let zh: Vec<BundleItem> = bundles
            .clone()
            .into_iter()
            .map(|b| BundleItem::from_native(b, "zh-CN"))
            .collect();
        assert_eq!(zh[0].title.as_deref(), Some("@deepseek-ai/dsh-base"));
        assert_eq!(zh[0].description.as_deref(), Some("The core"));
        assert_eq!(zh[1].title.as_deref(), Some("任务调度"));
        assert_eq!(zh[1].description.as_deref(), Some("保存任务"));
        assert_eq!(zh[2].title, None, "no meta at all stays absent");
        // The layer's size is a fact about the layer.
        assert_eq!(zh[0].row_count, 2);
        assert_eq!(zh[2].row_count, 0);

        // The same list, shown in English.
        let en: Vec<BundleItem> = bundles
            .into_iter()
            .map(|b| BundleItem::from_native(b, "en-US"))
            .collect();
        assert_eq!(en[1].title.as_deref(), Some("Task scheduling"));
    }

    /// Locale resolution: exact tag, then the tag's base language, then the
    /// service's own `en`, then whatever the map holds — and an empty result
    /// reads as "no text" rather than a blank line.
    #[test]
    fn native_text_picks_a_locale_with_fallbacks() {
        let plain: NativeText = serde_json::from_value(serde_json::json!("Text")).unwrap();
        assert_eq!(plain.pick("zh-CN"), "Text");

        let map: NativeText =
            serde_json::from_value(serde_json::json!({ "en": "Hello", "zh": "你好" })).unwrap();
        assert_eq!(map.pick("zh-CN"), "你好");
        assert_eq!(map.pick("zh-TW"), "你好", "a base language matches its regions");
        assert_eq!(map.pick("en-GB"), "Hello");
        assert_eq!(map.pick("fr"), "Hello", "`en` is the service's own fallback");

        let only_other: NativeText =
            serde_json::from_value(serde_json::json!({ "fr": "Bonjour" })).unwrap();
        assert_eq!(only_other.pick("zh-CN"), "Bonjour");

        let blank: NativeText =
            serde_json::from_value(serde_json::json!({ "fr": "   " })).unwrap();
        assert_eq!(blank.pick_opt("zh-CN"), None);
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
                block_on(client.list_bundles()).is_err(),
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

    /// `setBundleEnabled` sends the verified envelope: the bare method name
    /// (the `pluginManager/` prefix is added in the envelope) and the flat
    /// `{name, enabled}` args — the parameter is `name`, **not** `id`.
    #[test]
    fn set_bundle_enabled_sends_name_and_enabled() {
        let (client, fake) = client_with(vec![ok_response(serde_json::json!({
            "changed": true,
            "application": "applied",
            "stage": "enable",
            "target": "@deepseek-ai/dsh-web-app"
        }))]);
        let outcome = block_on(client.set_bundle_enabled("@deepseek-ai/dsh-web-app", false)).unwrap();

        let req = fake.last_sent();
        assert_eq!(req.envelope.method, "pluginManager/setBundleEnabled");
        assert_eq!(
            req.url,
            "http://127.0.0.1:34567/api/pluginManager/setBundleEnabled"
        );
        assert_eq!(
            req.envelope.payload.args,
            serde_json::json!({ "name": "@deepseek-ai/dsh-web-app", "enabled": false })
        );
        assert!(outcome.changed);
        assert_eq!(outcome.application, "applied");
        assert_eq!(outcome.stage, "enable");
        assert_eq!(outcome.target, "@deepseek-ai/dsh-web-app");
    }

    /// A refused operation is HTTP 200 + `ok:true` with the refusal inside
    /// `value`: it deserializes into a result (not an `Err`), carrying
    /// `changed:false` and the `error.code` the UI translates.
    #[test]
    fn set_bundle_enabled_parses_a_refusal_as_a_result_not_an_error() {
        let (client, _) = client_with(vec![ok_response(serde_json::json!({
            "changed": false,
            "application": "failed",
            "stage": "enable",
            "target": "@deepseek-ai/dsh-base",
            "error": { "code": "management-required" },
            "warnings": ["随 DSH 发布"]
        }))]);
        let outcome = block_on(client.set_bundle_enabled("@deepseek-ai/dsh-base", false)).unwrap();
        assert!(!outcome.changed);
        assert_eq!(outcome.application, "failed");
        assert_eq!(outcome.error.as_ref().unwrap().code, "management-required");
        assert_eq!(outcome.warnings, vec!["随 DSH 发布".to_string()]);
    }

    /// The read-only snapshot carries the mode + localized reason, no instance
    /// id, and whatever the profile's files said (the caller passes the
    /// projection; the shape only has to survive serialization).
    #[test]
    fn readonly_view_carries_the_mode_reason_and_file_projection() {
        let view = PluginView::readonly(
            PluginViewMode::NotRunning,
            "实例未运行",
            vec![BundleItem::from_profile_files(
                "ds-bot".to_string(),
                true,
                true,
                Some("0.9.4".to_string()),
            )],
        );
        assert_eq!(view.mode, PluginViewMode::NotRunning);
        assert_eq!(view.reason.as_deref(), Some("实例未运行"));
        assert!(view.instance_id.is_none());
        assert_eq!(view.bundles.len(), 1);
        assert_eq!(view.bundles[0].name, "ds-bot");
        assert!(view.bundles[0].enabled);
        assert!(!view.bundles[0].removable, "a file projection proves nothing about removal");
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["mode"], "not-running");
        assert!(json["instanceId"].is_null());
        assert_eq!(json["bundles"][0]["version"], "0.9.4");
    }
    /// Projection: a bundle's `meta.title` becomes `title` (resolved to one
    /// language), its own `description` outranks the meta one, and its error
    /// code — not its diagnostic — becomes `errorCode`.
    #[test]
    fn managed_view_projects_meta_and_bundle_error() {
        let bundles = serde_json::from_value(serde_json::json!([{
            "name": "ds-bot",
            "enabled": false,
            "installed": true,
            "removable": true,
            "optional": false,
            "error": { "code": "bundle-in-use" },
            "meta": { "title": { "en": "Bot", "zh": "机器人" } }
        }]))
        .unwrap();
        let view = PluginView::managed(bundles, "zh-CN", "i-main");
        assert_eq!(view.mode, PluginViewMode::Managed);
        assert_eq!(view.instance_id.as_deref(), Some("i-main"));
        assert_eq!(view.bundles[0].name, "ds-bot");
        assert_eq!(view.bundles[0].title.as_deref(), Some("机器人"));
        assert_eq!(view.bundles[0].error_code.as_deref(), Some("bundle-in-use"));

        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["mode"], "managed");
        assert_eq!(json["instanceId"], "i-main");
        assert_eq!(json["bundles"][0]["name"], "ds-bot");
        assert_eq!(json["bundles"][0]["rowCount"], 0);
    }

    /// A native `ChangeResult` value as the service answers it.
    fn change_result(
        application: &str,
        changed: bool,
        error: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let mut value = serde_json::json!({
            "changed": changed,
            "application": application,
            "stage": "enable",
            "target": "include:llm",
            "warnings": [],
        });
        if let Some(err) = error {
            value["error"] = err;
        }
        value
    }

    /// `application` has five values, not three: each is carried through
    /// verbatim so the page can tell 已生效 / 需重启 / 被覆盖 / 失败 / 取消
    /// apart. Driven through the bundle toggle, the write path that remains.
    #[test]
    fn set_bundle_enabled_keeps_every_application_value() {
        for application in [
            "applied",
            "restart-required",
            "overridden",
            "failed",
            "cancelled",
        ] {
            let mut value = change_result(application, true, None);
            value["warnings"] = serde_json::json!(["w1"]);
            let (client, _) = client_with(vec![ok_response(value)]);
            let out = block_on(client.set_bundle_enabled("ds-bot", true)).unwrap();
            assert_eq!(out.application, application);
            assert_eq!(out.enabled, None);
            assert_eq!(out.warnings, vec!["w1".to_string()]);
        }
    }

    /// `removeBundle` sends the verified envelope: the bare method name (the
    /// `pluginManager/` prefix is added in the envelope) and the flat `{name}`
    /// args — removal takes only the bundle name.
    #[test]
    fn remove_bundle_sends_the_name_argument() {
        let (client, fake) = client_with(vec![ok_response(serde_json::json!({
            "changed": true,
            "application": "applied",
            "stage": "remove",
            "target": "ds-bot"
        }))]);
        let outcome = block_on(client.remove_bundle("ds-bot")).unwrap();

        let req = fake.last_sent();
        assert_eq!(req.url, "http://127.0.0.1:34567/api/pluginManager/removeBundle");
        assert_eq!(req.envelope.method, "pluginManager/removeBundle");
        assert_eq!(req.envelope.payload.args, serde_json::json!({ "name": "ds-bot" }));
        assert!(outcome.changed);
        assert_eq!(outcome.application, "applied");
        assert_eq!(outcome.stage, "remove");
        assert_eq!(outcome.target, "ds-bot");
    }

    /// Removing a bundle that ships with DSH (`removable:false`) is refused as
    /// HTTP 200 + `result.ok:true` with `changed:false` and
    /// `error.code:"not-removable"` — an ordinary result the page explains,
    /// never an `Err`. This is the common case, not an edge case.
    #[test]
    fn remove_bundle_parses_a_not_removable_refusal_as_a_result() {
        let (client, _) = client_with(vec![ok_response(serde_json::json!({
            "changed": false,
            "application": "failed",
            "stage": "remove",
            "target": "@deepseek-ai/dsh-base",
            "error": { "code": "not-removable" }
        }))]);
        let outcome = block_on(client.remove_bundle("@deepseek-ai/dsh-base")).unwrap();
        assert!(!outcome.changed);
        assert_eq!(outcome.application, "failed");
        assert_eq!(outcome.stage, "remove");
        assert_eq!(outcome.error.as_ref().unwrap().code, "not-removable");
    }
}
