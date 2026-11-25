use crate::config::LauncherSettings;
use std::sync::RwLock;

#[derive(Clone, Debug)]
struct ProxyConf {
    server: String,
    no_proxy: String,
}

fn parse_proxy_url(url: &str, port: u16) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let explicit_port = has_explicit_port(trimmed);
    let mut parsed = reqwest::Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    // `Url::port()` reports `None` for a scheme's default port even when the
    // URL spelled it out, so the raw text decides whether a port was given.
    if !explicit_port {
        parsed.set_port(Some(port)).ok()?;
    }
    // reqwest renders a bare host with a trailing slash; the environment
    Some(parsed.as_str().trim_end_matches('/').to_string())
}

fn has_explicit_port(url: &str) -> bool {
    let Some((_, after_scheme)) = url.split_once("://") else {
        return false;
    };
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    match authority.rsplit_once(':') {
        Some((_, port)) => !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

fn conf_from_settings(s: &LauncherSettings) -> Option<ProxyConf> {
    if !s.proxy_enabled {
        return None;
    }
    let server = parse_proxy_url(&s.proxy_url, s.proxy_port)?;
    Some(ProxyConf {
        server,
        no_proxy: s.no_proxy.trim().to_string(),
    })
}

pub fn validate_settings(settings: &LauncherSettings) -> Result<(), String> {
    if !settings.proxy_enabled {
        return Ok(());
    }
    match parse_proxy_url(&settings.proxy_url, settings.proxy_port) {
        Some(_) => Ok(()),
        None => Err(format!(
            "代理地址无效: {}（需以 http:// 或 https:// 开头）",
            settings.proxy_url
        )),
    }
}

/// Snapshot of the active proxy for HTTP client construction sites that have
/// no access to the config mutex. Synced at startup and on every settings
/// update.
static CURRENT: RwLock<Option<ProxyConf>> = RwLock::new(None);

pub fn sync_from_settings(settings: &LauncherSettings) {
    *CURRENT.write().unwrap() = conf_from_settings(settings);
}

pub fn apply(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let guard = CURRENT.read().unwrap();
    let Some(conf) = guard.as_ref() else {
        return builder;
    };
    match reqwest::Proxy::all(&conf.server) {
        Ok(proxy) => builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_string(&conf.no_proxy))),
        Err(e) => {
            crate::log_warn!("代理地址无效，忽略代理设置 {}: {e}", conf.server);
            builder
        }
    }
}

const PROXY_KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// Injects the proxy into an instance environment, replacing any proxy
/// variables the instance already defines (both cases). Callers check
/// `proxy_enabled && proxy_apply_dsh` first.
pub fn override_env(env: &mut Vec<(String, String)>, settings: &LauncherSettings) {
    let Some(conf) = conf_from_settings(settings) else {
        return;
    };
    env.retain(|(k, _)| !PROXY_KEYS.iter().any(|p| p.eq_ignore_ascii_case(k)));
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        env.push((key.to_string(), conf.server.clone()));
    }
    env.push(("NO_PROXY".to_string(), conf.no_proxy.clone()));
    env.push(("no_proxy".to_string(), conf.no_proxy));
}

pub fn apply_to_command(cmd: &mut tokio::process::Command) {
    let guard = CURRENT.read().unwrap();
    let Some(conf) = guard.as_ref() else {
        return;
    };
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        cmd.env(key, &conf.server);
    }
    cmd.env("NO_PROXY", &conf.no_proxy);
    cmd.env("no_proxy", &conf.no_proxy);
}
