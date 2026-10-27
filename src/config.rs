use std::path::PathBuf;

use serde::Deserialize;

/// User configuration, `config.toml` in the platform config dir.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub external: External,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct External {
    /// Binary called on `e` with `<anime|manga> <id>`.
    pub command: Option<PathBuf>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("aniscrobble").join("config.toml"))
    }

    /// A missing file is not an error: the defaults apply. A broken file
    /// returns the defaults plus the reason, so the TUI can show it.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::path() else {
            return (Self::default(), None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(err) => {
                return (
                    Self::default(),
                    Some(format!("cannot read {}: {err}", path.display())),
                );
            }
        };
        match toml::from_str(&text) {
            Ok(config) => (config, None),
            Err(err) => (
                Self::default(),
                Some(format!("invalid {}: {err}", path.display())),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_command_is_parsed() {
        let config: Config = toml::from_str("[external]\ncommand = \"/bin/notify\"\n").unwrap();
        assert_eq!(config.external.command, Some(PathBuf::from("/bin/notify")));
    }

    #[test]
    fn empty_file_uses_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.external.command.is_none());
    }
}
