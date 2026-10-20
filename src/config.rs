//! User configuration loaded from `config.toml`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

/// Default time after which an unseen device is removed.
pub const DEFAULT_DEVICE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(with = "humantime_serde")]
    pub device_timeout: Duration,
    pub interfaces: InterfaceConfig,
    pub ipv6: bool,
    /// Whether mouse capture is on at startup.
    pub mouse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InterfaceConfig {
    /// Glob patterns; empty means automatic selection.
    pub include: Vec<String>,
    /// Glob patterns; replaces the default list when set.
    pub exclude: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_timeout: DEFAULT_DEVICE_TIMEOUT,
            interfaces: InterfaceConfig::default(),
            ipv6: true,
            mouse: true,
        }
    }
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            // Container veth pairs, and macOS links that never carry LAN
            // mDNS: AWDL (awdl, llw), tunnels (utun) and anpi.
            exclude: ["veth*", "awdl*", "llw*", "utun*", "anpi*"]
                .map(String::from)
                .to_vec(),
        }
    }
}

impl Config {
    /// Parses configuration text; `origin` is only used in error messages.
    pub fn parse(text: &str, origin: &Path) -> Result<Self> {
        let config: Config = toml::from_str(text)
            .map_err(|e| anyhow!("invalid configuration in {}: {e}", origin.display()))?;
        config.validate(origin)?;
        Ok(config)
    }

    fn validate(&self, origin: &Path) -> Result<()> {
        if self.device_timeout.is_zero() {
            return Err(anyhow!(
                "invalid configuration in {}: device_timeout must be greater than zero",
                origin.display()
            ));
        }
        for (key, patterns) in [
            ("interfaces.include", &self.interfaces.include),
            ("interfaces.exclude", &self.interfaces.exclude),
        ] {
            for pattern in patterns {
                globset::Glob::new(pattern).map_err(|e| {
                    anyhow!(
                        "invalid configuration in {}: {key}: bad pattern {pattern:?}: {e}",
                        origin.display()
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Loads the configuration from `explicit` or the platform default path.
    ///
    /// A missing default file yields the defaults; a missing explicit file is an error.
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        match explicit {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read configuration {}", path.display()))?;
                Self::parse(&text, path)
            }
            None => match default_path() {
                Some(path) if path.exists() => {
                    let text = std::fs::read_to_string(&path).with_context(|| {
                        format!("cannot read configuration {}", path.display())
                    })?;
                    Self::parse(&text, &path)
                }
                _ => Ok(Self::default()),
            },
        }
    }
}

/// `config.toml` in the platform configuration directory.
pub fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "mdnsview").map(|d| d.config_dir().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        Config::parse(text, Path::new("/test/config.toml"))
    }

    #[test]
    fn empty_file_gives_defaults() {
        let c = parse("").unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.device_timeout, Duration::from_secs(300));
        assert!(c.interfaces.include.is_empty());
        assert_eq!(
            c.interfaces.exclude,
            vec!["veth*", "awdl*", "llw*", "utun*", "anpi*"]
        );
        assert!(c.ipv6);
        assert!(c.mouse);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = parse(r#"device_timeout = "10m""#).unwrap();
        assert_eq!(c.device_timeout, Duration::from_secs(600));
        assert_eq!(c.interfaces, InterfaceConfig::default());
        assert!(c.ipv6);
        assert!(c.mouse);
    }

    #[test]
    fn mouse_can_be_disabled() {
        assert!(!parse("mouse = false").unwrap().mouse);
    }

    #[test]
    fn exclude_replaces_default_list() {
        let c = parse("[interfaces]\nexclude = [\"docker*\"]\n").unwrap();
        assert_eq!(c.interfaces.exclude, vec!["docker*"]);
    }

    #[test]
    fn bad_duration_names_setting_and_file() {
        let err = parse(r#"device_timeout = "five minutes""#).unwrap_err().to_string();
        assert!(err.contains("device_timeout"), "{err}");
        assert!(err.contains("/test/config.toml"), "{err}");
    }

    #[test]
    fn unknown_key_is_rejected() {
        let err = parse("colour = \"blue\"").unwrap_err().to_string();
        assert!(err.contains("colour"), "{err}");
    }

    #[test]
    fn unknown_nested_key_is_rejected() {
        let err = parse("[interfaces]\nonly = []\n").unwrap_err().to_string();
        assert!(err.contains("only"), "{err}");
    }

    #[test]
    fn bad_glob_is_rejected() {
        let err = parse("[interfaces]\ninclude = [\"[\"]\n").unwrap_err().to_string();
        assert!(err.contains("interfaces.include"), "{err}");
    }

    #[test]
    fn explicit_missing_file_is_error() {
        assert!(Config::load(Some(Path::new("/nonexistent/mdnsview.toml"))).is_err());
    }

    #[test]
    fn explicit_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.toml");
        std::fs::write(&path, "ipv6 = false\n").unwrap();
        let c = Config::load(Some(&path)).unwrap();
        assert!(!c.ipv6);
    }

    #[test]
    fn readme_example_parses() {
        let readme = include_str!("../README.md");
        let start = readme.find("```toml\n").expect("README has a toml example") + 8;
        let end = start + readme[start..].find("```").unwrap();
        let c = parse(&readme[start..end]).unwrap();
        assert_eq!(c.device_timeout, Duration::from_secs(300));
        assert!(c.mouse);
        assert_eq!(c.interfaces.exclude, InterfaceConfig::default().exclude);
    }
}
