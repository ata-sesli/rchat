use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KittyHostPreferences {
    pub executable_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyPathSource {
    Configured,
    Environment,
    Sibling,
    Path,
    Application,
}

impl KittyPathSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Environment => "environment",
            Self::Sibling => "sibling",
            Self::Path => "PATH",
            Self::Application => "application",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKitty {
    pub path: PathBuf,
    pub source: KittyPathSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryResult {
    pub resolved: Option<ResolvedKitty>,
    pub warning: Option<String>,
}

#[derive(Debug)]
pub enum LaunchOutcome {
    RunHere { warning: Option<String> },
    HostedExited(ExitStatus),
}

pub fn preferences_path(app_dir: &Path) -> PathBuf {
    app_dir.join("kitty-host.json")
}

pub fn load_preferences(app_dir: &Path) -> Result<KittyHostPreferences> {
    let path = preferences_path(app_dir);
    if !path.is_file() {
        return Ok(KittyHostPreferences::default());
    }
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "failed to read Kitty host preferences at {}",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "failed to parse Kitty host preferences at {}",
            path.display()
        )
    })
}

pub fn save_preferences(app_dir: &Path, preferences: &KittyHostPreferences) -> Result<()> {
    fs::create_dir_all(app_dir).with_context(|| {
        format!(
            "failed to create application data directory {}",
            app_dir.display()
        )
    })?;
    let path = preferences_path(app_dir);
    let bytes = serde_json::to_vec_pretty(preferences)?;
    fs::write(&path, bytes).with_context(|| {
        format!(
            "failed to write Kitty host preferences at {}",
            path.display()
        )
    })
}

fn kitty_file_name() -> &'static str {
    if cfg!(windows) {
        "kitty.exe"
    } else {
        "kitty"
    }
}

fn file_candidate(path: PathBuf, source: KittyPathSource) -> Option<ResolvedKitty> {
    path.is_file().then_some(ResolvedKitty { path, source })
}

fn resolve_kitty_executable(
    configured: Option<&Path>,
    environment: Option<&OsStr>,
    current_exe: &Path,
    path: Option<&OsStr>,
) -> DiscoveryResult {
    let mut warning = None;
    if let Some(configured) = configured.filter(|path| !path.as_os_str().is_empty()) {
        if let Some(resolved) =
            file_candidate(configured.to_path_buf(), KittyPathSource::Configured)
        {
            return DiscoveryResult {
                resolved: Some(resolved),
                warning,
            };
        }
        warning = Some(format!(
            "configured Kitty executable was not found: {}",
            configured.display()
        ));
    }

    if let Some(environment) = environment.filter(|value| !value.is_empty()) {
        if let Some(resolved) =
            file_candidate(PathBuf::from(environment), KittyPathSource::Environment)
        {
            return DiscoveryResult {
                resolved: Some(resolved),
                warning,
            };
        }
    }

    if let Some(parent) = current_exe.parent() {
        if let Some(resolved) =
            file_candidate(parent.join(kitty_file_name()), KittyPathSource::Sibling)
        {
            return DiscoveryResult {
                resolved: Some(resolved),
                warning,
            };
        }
    }

    if let Some(path) = path {
        for directory in env::split_paths(path) {
            if let Some(resolved) =
                file_candidate(directory.join(kitty_file_name()), KittyPathSource::Path)
            {
                return DiscoveryResult {
                    resolved: Some(resolved),
                    warning,
                };
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut applications = vec![PathBuf::from(
            "/Applications/kitty.app/Contents/MacOS/kitty",
        )];
        if let Some(home) = env::var_os("HOME") {
            applications
                .push(PathBuf::from(home).join("Applications/kitty.app/Contents/MacOS/kitty"));
        }
        for candidate in applications {
            if let Some(resolved) = file_candidate(candidate, KittyPathSource::Application) {
                return DiscoveryResult {
                    resolved: Some(resolved),
                    warning,
                };
            }
        }
    }
    DiscoveryResult {
        resolved: None,
        warning,
    }
}

pub fn resolved_kitty(app_dir: &Path) -> Result<DiscoveryResult> {
    let preferences = load_preferences(app_dir)?;
    let current_exe =
        env::current_exe().context("failed to resolve the current RChat executable")?;
    Ok(resolve_kitty_executable(
        preferences.executable_path.as_deref(),
        env::var_os("RCHAT_KITTY_PATH").as_deref(),
        &current_exe,
        env::var_os("PATH").as_deref(),
    ))
}

pub fn is_kitty_session() -> bool {
    kitty_session_from_environment(
        env::var_os("KITTY_WINDOW_ID").is_some(),
        env::var("RCHAT_KITTY_SESSION").ok().as_deref(),
        env::var("TERM").ok().as_deref(),
    )
}

fn kitty_session_from_environment(
    window_id: bool,
    managed: Option<&str>,
    term: Option<&str>,
) -> bool {
    // SSH forwards TERM when allocating a PTY, unlike Kitty's local window ID.
    window_id || managed == Some("1") || term == Some("xterm-kitty")
}

fn should_run_here(hosted: bool, bypass: bool, remote: bool, help: bool) -> bool {
    hosted || bypass || remote || help
}

pub fn build_kitty_args(current_exe: &Path, child_args: &[OsString]) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--title"),
        OsString::from("RChat"),
        current_exe.as_os_str().to_owned(),
    ];
    args.extend_from_slice(child_args);
    args
}

pub fn launch_if_needed(args: &[OsString]) -> Result<LaunchOutcome> {
    let bypass = args.iter().any(|arg| arg == OsStr::new("--no-host"))
        || env::var("RCHAT_NO_HOST").ok().as_deref() == Some("1");
    let remote = env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some();
    let help = args.iter().any(|arg| {
        ["--help", "-h", "--version", "-V"]
            .iter()
            .any(|flag| arg == OsStr::new(flag))
    });
    if should_run_here(is_kitty_session(), bypass, remote, help) {
        return Ok(LaunchOutcome::RunHere { warning: None });
    }
    let current_exe = env::current_exe().context("failed to resolve RChat executable")?;
    let app_dir = rchat_core::runtime::default_app_data_dir()?;
    let discovery = resolved_kitty(&app_dir)?;
    let Some(resolved) = discovery.resolved else {
        anyhow::bail!("Kitty was not found. Install Kitty or set RCHAT_KITTY_PATH to its executable. Use --no-host to run in the current terminal.");
    };
    if let Some(warning) = discovery.warning {
        eprintln!("{warning}");
    }
    let status = Command::new(&resolved.path)
        .args(build_kitty_args(&current_exe, &args[1..]))
        .env("RCHAT_KITTY_SESSION", "1")
        .status()
        .with_context(|| format!("failed to start Kitty at {}", resolved.path.display()))?;
    Ok(LaunchOutcome::HostedExited(status))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn kitty_session_recognizes_ssh_term_without_local_markers() {
        assert!(kitty_session_from_environment(
            false,
            None,
            Some("xterm-kitty")
        ));
        assert!(!kitty_session_from_environment(
            false,
            None,
            Some("xterm-256color")
        ));
        assert!(!kitty_session_from_environment(false, None, None));
        assert!(kitty_session_from_environment(true, None, None));
        assert!(kitty_session_from_environment(false, Some("1"), None));
    }

    fn touch(path: PathBuf) -> PathBuf {
        fs::write(&path, b"").unwrap();
        path
    }

    #[test]
    fn kitty_host_discovery_prefers_configured_then_environment_then_sibling_then_path() {
        let temp = tempfile::tempdir().unwrap();
        let configured = touch(temp.path().join("configured-kitty"));
        let env_override = touch(temp.path().join("env-kitty"));
        let sibling = touch(temp.path().join(kitty_file_name()));
        let path_dir = temp.path().join("bin");
        fs::create_dir(&path_dir).unwrap();
        let path_kitty = touch(path_dir.join(kitty_file_name()));
        let current = temp.path().join("rchat-tui");

        let configured_result = resolve_kitty_executable(
            Some(&configured),
            Some(env_override.as_os_str()),
            &current,
            Some(path_dir.as_os_str()),
        );
        assert_eq!(
            configured_result.resolved,
            Some(ResolvedKitty {
                path: configured,
                source: KittyPathSource::Configured,
            })
        );

        let environment_result = resolve_kitty_executable(
            None,
            Some(env_override.as_os_str()),
            &current,
            Some(path_dir.as_os_str()),
        );
        assert_eq!(
            environment_result.resolved.unwrap().source,
            KittyPathSource::Environment
        );

        let sibling_result =
            resolve_kitty_executable(None, None, &current, Some(path_dir.as_os_str()));
        assert_eq!(sibling_result.resolved.unwrap().path, sibling);

        fs::remove_file(sibling).unwrap();
        let path_result =
            resolve_kitty_executable(None, None, &current, Some(path_dir.as_os_str()));
        assert_eq!(path_result.resolved.unwrap().path, path_kitty);
    }

    #[test]
    fn kitty_host_invalid_configured_path_warns_while_using_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let fallback = touch(temp.path().join("fallback-kitty"));
        let current = temp.path().join("rchat-tui");
        let missing = temp.path().join("missing-kitty");

        let result =
            resolve_kitty_executable(Some(&missing), Some(fallback.as_os_str()), &current, None);

        assert_eq!(result.resolved.unwrap().path, fallback);
        assert!(result
            .warning
            .unwrap()
            .contains(&missing.display().to_string()));
    }

    #[test]
    fn kitty_host_avoids_nested_remote_and_help_launches() {
        assert!(should_run_here(true, false, false, false));
        assert!(should_run_here(false, true, false, false));
        assert!(should_run_here(false, false, true, false));
        assert!(should_run_here(false, false, false, true));
        assert!(!should_run_here(false, false, false, false));
    }

    #[test]
    fn kitty_host_forwards_arguments_without_shell_interpretation() {
        assert_eq!(
            build_kitty_args(
                Path::new("/some path/rchat-tui"),
                &[OsString::from("media-smoke")]
            ),
            ["--title", "RChat", "/some path/rchat-tui", "media-smoke"].map(OsString::from)
        );
    }

    #[test]
    fn kitty_host_preferences_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let preferences = KittyHostPreferences {
            executable_path: Some(PathBuf::from("/tmp/kitty")),
        };

        save_preferences(temp.path(), &preferences).unwrap();

        assert_eq!(load_preferences(temp.path()).unwrap(), preferences);
        assert_eq!(
            preferences_path(temp.path()),
            temp.path().join("kitty-host.json")
        );
    }
}
