//! Configuration: `~/.config/drift/config.toml` (or `$XDG_CONFIG_HOME`),
//! with `[keys]` and `[theme]` sections. Missing file → defaults; invalid
//! file → a startup error naming what's wrong. `--init-config` writes the
//! full default file to edit from.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::keymap::{KEY_DEFAULTS, Keymap};
use crate::theme::{THEME_DEFAULTS, Theme};

/// Default editor command: `{line}` and `{file}` are substituted; the
/// file path is appended when `{file}` doesn't appear.
pub const EDITOR_DEFAULT: &str = "nvim +{line}";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    /// Default base branch; the `--base` flag overrides it, and without
    /// either the base is auto-detected (origin/HEAD, main, master).
    #[serde(default)]
    base: Option<String>,
    /// Editor command for the open-in-editor action.
    #[serde(default)]
    editor: Option<String>,
    #[serde(default)]
    keys: HashMap<String, Vec<String>>,
    #[serde(default)]
    theme: HashMap<String, String>,
}

pub struct Config {
    pub base: Option<String>,
    pub editor: String,
    pub keymap: Keymap,
    pub theme: Theme,
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
            home.join(".config")
        });
    base.join("drift").join("config.toml")
}

pub fn load() -> Result<Config> {
    let path = config_path();
    let file: ConfigFile = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid config at {}", path.display()))?,
        Err(_) => ConfigFile::default(),
    };
    let keymap = Keymap::from_overrides(&file.keys)
        .with_context(|| format!("invalid [keys] in {}", path.display()))?;
    let theme = Theme::from_overrides(&file.theme)
        .with_context(|| format!("invalid [theme] in {}", path.display()))?;
    Ok(Config {
        base: file.base,
        editor: file.editor.unwrap_or_else(|| EDITOR_DEFAULT.to_string()),
        keymap,
        theme,
    })
}

/// The full default configuration in file syntax, generated from the same
/// tables the runtime uses.
pub fn default_toml() -> String {
    let mut out = String::from(
        "# drift configuration\n\
         #\n\
         # keys: single characters (\"g\", \"G\", \"<\"), named keys (enter,\n\
         # space, tab, up, down, left, right, pageup, pagedown, home, end),\n\
         # optionally prefixed \"ctrl-\". Digits and esc are reserved.\n\
         # Listing an action replaces all of its default keys.\n\
         #\n\
         # theme: ANSI names (\"darkgray\"), 256-color indexes (\"114\"),\n\
         # or hex (\"#87d787\").\n\n\
         # Default base branch (--base overrides; auto-detected if unset).\n\
         # base = \"main\"\n\n\
         # Editor for the open-in-editor key. {file} and {line} are\n\
         # substituted; the file path is appended when {file} is absent.\n\
         #   editor = \"code -g {file}:{line}\"\n\
         #   editor = \"subl {file}:{line}\"\n\
         editor = \"nvim +{line}\"\n\n[keys]\n",
    );
    for (name, _, keys) in KEY_DEFAULTS {
        let list = keys
            .iter()
            .map(|k| format!("\"{k}\""))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("{name} = [{list}]\n"));
    }
    out.push_str("\n[theme]\n");
    for (name, value) in THEME_DEFAULTS {
        out.push_str(&format!("{name} = \"{value}\"\n"));
    }
    out
}

/// Write the default config file; refuses to overwrite an existing one.
pub fn write_default() -> Result<PathBuf> {
    let path = config_path();
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, default_toml())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_toml_round_trips() {
        let file: ConfigFile = toml::from_str(&default_toml()).unwrap();
        assert!(Keymap::from_overrides(&file.keys).is_ok());
        assert!(Theme::from_overrides(&file.theme).is_ok());
    }
}
