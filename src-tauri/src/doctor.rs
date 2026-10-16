//! Dependency-tree preflight for an instance + profile.
//!
//! DSH's core packages are provided by the CLI's own dependency tree; a
//! profile must never carry its own copy. Two states are therefore treated as
//! faults:
//!
//! 1. the profile's `node_modules` contains any `@deepseek-ai/*` core package;
//! 2. two generations of core are mixed (the copy inside the profile has a
//!    different version than the core in the CLI tree).
//!
//! (2) is the silent failure mode: two copies of a core package mint two
//! unequal module-local `Symbol()`s, the agent loop's scheduler lookup on
//! ToolRuntime returns `undefined`, and every tool call in that profile dies
//! in `.prepare` with no load-time error and no hint about which package was
//! duplicated. A launcher that creates many instances/versions is the most
//! likely producer of that state, so it checks for it before starting.
//!
//! The report is advisory: findings are logged and surfaced in the UI, never
//! used to block a launch.

use serde::Serialize;
use std::path::Path;

pub const HEALTH_EVENT: &str = "instance://health";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingLevel {
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorFinding {
    pub level: FindingLevel,
    /// Stable machine-readable code (also used as the i18n key suffix).
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub instance_id: String,
    pub profile: String,
    pub findings: Vec<DoctorFinding>,
}

/// Reads a package's version from `<dir>/package.json`.
fn package_version(pkg_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;
    doc.get("version")?.as_str().map(|s| s.to_string())
}

/// Version of the DSH CLI package inside an installed version tree. Source
/// checkouts (GitHub-only tags) keep it at the workspace path.
fn cli_core_version(version_dir: &Path) -> Option<String> {
    package_version(
        &version_dir
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh"),
    )
    .or_else(|| package_version(&version_dir.join("apps").join("cli")))
}

/// Checks if a package name is a DSH/Cordis core runtime package.
/// Utility libraries (cosmokit, schemastery, etc.) are independent libraries
/// commonly imported by plugins and do not participate in DSH singleton/scheduler state.
fn is_core_package(name: &str) -> bool {
    let short = name.strip_prefix("@deepseek-ai/").unwrap_or(name);
    short == "dsh" || short.starts_with("dsh-") || short == "cordis" || short.starts_with("cordis-")
}

/// Version of a package inside the CLI's dependency tree.
fn cli_package_version(version_dir: &Path, pkg_name: &str) -> Option<String> {
    let short = pkg_name.strip_prefix("@deepseek-ai/").unwrap_or(pkg_name);

    // 1. Direct path in node_modules/@deepseek-ai/<name>
    if let Some(v) = package_version(
        &version_dir
            .join("node_modules")
            .join("@deepseek-ai")
            .join(short),
    ) {
        return Some(v);
    }

    // 2. Monorepo source checkout: packages/<name> or apps/<name>
    if let Some(v) = package_version(&version_dir.join("packages").join(short)) {
        return Some(v);
    }
    if let Some(v) = package_version(&version_dir.join("apps").join(short)) {
        return Some(v);
    }

    // 3. pnpm virtual store: node_modules/.pnpm/@deepseek-ai+<short>@<version>...
    let pnpm_dir = version_dir.join("node_modules").join(".pnpm");
    if let Ok(entries) = std::fs::read_dir(&pnpm_dir) {
        let prefix = format!("@deepseek-ai+{short}@");
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) {
                let pkg_path = entry
                    .path()
                    .join("node_modules")
                    .join("@deepseek-ai")
                    .join(short);
                if let Some(v) = package_version(&pkg_path) {
                    return Some(v);
                }
            }
        }
    }

    // 4. Fallback for dsh / dsh-* monorepo packages that share the dsh CLI version:
    if short == "dsh" || short.starts_with("dsh-") {
        return cli_core_version(version_dir);
    }

    None
}

/// Every `@deepseek-ai/*` package that has a copy inside the profile's
/// node_modules, as (package id, version) pairs. Normally empty: core comes
/// from the CLI tree, and the launcher never adds core packages to a profile.
fn profile_core_copies(profile_dir: &Path) -> Vec<(String, Option<String>)> {
    let scope = profile_dir.join("node_modules").join("@deepseek-ai");
    let Ok(entries) = std::fs::read_dir(&scope) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Option<String>)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| is_core_package(&e.file_name().to_string_lossy()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (format!("@deepseek-ai/{name}"), package_version(&e.path()))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Runs both checks. `selected_version` is the version string the instance is
/// configured to run (as recorded by the launcher).
pub fn inspect(
    instance_id: &str,
    profile: &str,
    version_dir: &Path,
    selected_version: &str,
    profile_dir: &Path,
) -> DoctorReport {
    let mut findings = Vec::new();
    let cli_core = cli_core_version(version_dir);

    // 1. The CLI tree's core version must match what the user selected.
    match &cli_core {
        Some(actual) if actual != selected_version => findings.push(DoctorFinding {
            level: FindingLevel::Warn,
            code: "core-version-mismatch".to_string(),
            message: format!(
                "实例选择的 DSH 版本为 {selected_version}，但 CLI 依赖树中的 @deepseek-ai/dsh 实际为 {actual}"
            ),
        }),
        None => findings.push(DoctorFinding {
            level: FindingLevel::Warn,
            code: "core-missing".to_string(),
            message: format!(
                "未能在版本目录中读取 @deepseek-ai/dsh 的版本信息：{}",
                version_dir.display()
            ),
        }),
        _ => {}
    }

    // 2. A profile must not carry core copies; a version-mismatched copy is
    //    the two-generations state and gets escalated to an error.
    for (pkg, version) in profile_core_copies(profile_dir) {
        let cli_pkg_ver = cli_package_version(version_dir, &pkg);
        let mixed = match (&version, &cli_pkg_ver) {
            (Some(v), Some(core)) => v != core,
            _ => false,
        };
        let shown = version.clone().unwrap_or_else(|| "未知版本".to_string());
        if mixed {
            let core = cli_pkg_ver.clone().unwrap_or_default();
            findings.push(DoctorFinding {
                level: FindingLevel::Error,
                code: "profile-core-mixed".to_string(),
                message: format!(
                    "Profile「{profile}」的 node_modules 中存在核心包 {pkg}@{shown}，与 CLI 树中的 {core} 不同代；\
                     该 profile 的工具调用可能全部失败，请卸载该包后重装插件"
                ),
            });
        } else {
            findings.push(DoctorFinding {
                level: FindingLevel::Warn,
                code: "profile-core-copy".to_string(),
                message: format!(
                    "Profile「{profile}」的 node_modules 中存在核心包 {pkg}@{shown}；\
                     核心包应由 CLI 依赖树提供，profile 中不应出现"
                ),
            });
        }
    }

    DoctorReport {
        instance_id: instance_id.to_string(),
        profile: profile.to_string(),
        findings,
    }
}

/// Logs a report through the runtime log (errors as error, rest as warn).
pub fn log_report(report: &DoctorReport) {
    for f in &report.findings {
        match f.level {
            FindingLevel::Error => {
                crate::log_error!("[依赖自检] {} ({})", f.message, f.code)
            }
            FindingLevel::Warn => {
                crate::log_warn!("[依赖自检] {} ({})", f.message, f.code)
            }
        }
    }
    if report.findings.is_empty() {
        crate::log_debug!(
            "[依赖自检] 实例 {} / profile {} 未发现异常",
            report.instance_id,
            report.profile
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: std::path::PathBuf,
        version_dir: std::path::PathBuf,
        profile_dir: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("dsh-doctor-{}", uuid::Uuid::new_v4()));
            let version_dir = root.join("versions").join("0.1.1-rc.2");
            let profile_dir = root.join("home").join("profiles").join("web");
            std::fs::create_dir_all(&version_dir).unwrap();
            std::fs::create_dir_all(&profile_dir).unwrap();
            Self {
                root,
                version_dir,
                profile_dir,
            }
        }

        fn write_pkg(&self, dir: &Path, name: &str, version: &str) {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("package.json"),
                format!(r#"{{"name":"{name}","version":"{version}"}}"#),
            )
            .unwrap();
        }

        fn cli_core(&self, version: &str) {
            let dir = self
                .version_dir
                .join("node_modules")
                .join("@deepseek-ai")
                .join("dsh");
            self.write_pkg(&dir, "@deepseek-ai/dsh", version);
        }

        fn profile_core(&self, short_name: &str, version: &str) {
            let dir = self
                .profile_dir
                .join("node_modules")
                .join("@deepseek-ai")
                .join(short_name);
            self.write_pkg(&dir, &format!("@deepseek-ai/{short_name}"), version);
        }

        fn run(&self, selected: &str) -> DoctorReport {
            inspect("i-1", "web", &self.version_dir, selected, &self.profile_dir)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[test]
    fn healthy_tree_reports_nothing() {
        let fx = Fixture::new();
        fx.cli_core("0.1.1-rc.2");
        let report = fx.run("0.1.1-rc.2");
        assert!(
            report.findings.is_empty(),
            "unexpected findings: {:?}",
            report.findings
        );
    }

    #[test]
    fn selected_version_mismatch_is_reported() {
        let fx = Fixture::new();
        fx.cli_core("0.1.0-rc.8");
        let report = fx.run("0.1.1-rc.2");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "core-version-mismatch");
        assert_eq!(report.findings[0].level, FindingLevel::Warn);
    }

    #[test]
    fn missing_cli_core_is_reported() {
        let fx = Fixture::new();
        let report = fx.run("0.1.1-rc.2");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "core-missing");
    }

    #[test]
    fn same_generation_profile_copy_is_a_warning() {
        let fx = Fixture::new();
        fx.cli_core("0.1.1-rc.2");
        fx.profile_core("dsh-tools", "0.1.1-rc.2");
        let report = fx.run("0.1.1-rc.2");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "profile-core-copy");
        assert_eq!(report.findings[0].level, FindingLevel::Warn);
        assert!(report.findings[0]
            .message
            .contains("@deepseek-ai/dsh-tools"));
    }

    #[test]
    fn mixed_generation_profile_copy_is_an_error() {
        // The #4640 signature: CLI on 0.1.1-rc.2, a stale core copy in the
        // profile on 0.1.0-rc.8.
        let fx = Fixture::new();
        fx.cli_core("0.1.1-rc.2");
        fx.profile_core("dsh-tools", "0.1.0-rc.8");
        let report = fx.run("0.1.1-rc.2");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "profile-core-mixed");
        assert_eq!(report.findings[0].level, FindingLevel::Error);
    }

    #[test]
    fn empty_scope_dir_is_not_a_finding() {
        let fx = Fixture::new();
        fx.cli_core("0.1.1-rc.2");
        std::fs::create_dir_all(fx.profile_dir.join("node_modules").join("@deepseek-ai")).unwrap();
        let report = fx.run("0.1.1-rc.2");
        assert!(report.findings.is_empty());
    }

    #[test]
    fn utility_libraries_in_profile_are_not_flagged() {
        let fx = Fixture::new();
        fx.cli_core("0.1.1-rc.2");
        fx.profile_core("cosmokit", "1.8.3");
        fx.profile_core("schemastery", "3.18.2");
        let report = fx.run("0.1.1-rc.2");
        assert!(report.findings.is_empty());
    }
}
