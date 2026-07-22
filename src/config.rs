//! Configuration: `~/.config/drift/config.toml` (or `$XDG_CONFIG_HOME`),
//! with `[keys]` and `[theme]` sections. Missing file → defaults; invalid
//! file → a startup error naming what's wrong. `--init-config` writes the
//! full default file to edit from.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::keymap::{KEY_DEFAULTS, Keymap};
use crate::theme::{THEME_DEFAULTS, THEME_LANG_DEFAULTS, Theme};

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
    /// Base color theme: a built-in name or a file in `themes/` next to
    /// the config. `[theme]` entries override on top of it.
    #[serde(default)]
    colorscheme: Option<String>,
    /// Editor command for the open-in-editor action.
    #[serde(default)]
    editor: Option<String>,
    #[serde(default)]
    keys: HashMap<String, Vec<String>>,
    /// Flat color entries plus `[theme.<lang>]` per-language sub-tables,
    /// split apart in [`load`].
    #[serde(default)]
    theme: HashMap<String, toml::Value>,
}

/// Per-language theme sections: language name → key → color string.
type LangThemes = HashMap<String, HashMap<String, String>>;

/// Split the raw `[theme]` map into flat color entries and per-language
/// sections; anything else (numbers, arrays…) is a descriptive error.
fn split_theme(
    raw: &HashMap<String, toml::Value>,
) -> Result<(HashMap<String, String>, LangThemes)> {
    let mut flat = HashMap::new();
    let mut langs: LangThemes = HashMap::new();
    for (name, value) in raw {
        match value {
            toml::Value::String(color) => {
                flat.insert(name.clone(), color.clone());
            }
            toml::Value::Table(entries) => {
                let section = langs.entry(name.clone()).or_default();
                for (key, color) in entries {
                    let toml::Value::String(color) = color else {
                        bail!("theme.{name}.{key}: expected a color string");
                    };
                    section.insert(key.clone(), color.clone());
                }
            }
            _ => bail!("theme.{name}: expected a color string or [theme.{name}] table"),
        }
    }
    Ok((flat, langs))
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
            // HOME is absent on Windows; home_dir falls back to USERPROFILE.
            let home = std::env::home_dir().unwrap_or_default();
            home.join(".config")
        });
    base.join("drift").join("config.toml")
}

pub fn load() -> Result<Config> {
    load_at(&config_path())
}

fn load_at(path: &Path) -> Result<Config> {
    let file: ConfigFile = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid config at {}", path.display()))?,
        Err(_) => ConfigFile::default(),
    };
    let keymap = Keymap::from_overrides(&file.keys)
        .with_context(|| format!("invalid [keys] in {}", path.display()))?;
    // Colors layer: named colorscheme first, `[theme]` overrides on top.
    let (mut flat, mut langs) = resolve_colorscheme(
        file.colorscheme.as_deref().unwrap_or("onedark"),
        path.parent(),
    )?;
    let (user_flat, user_langs) = split_theme(&file.theme)
        .with_context(|| format!("invalid [theme] in {}", path.display()))?;
    flat.extend(user_flat);
    for (lang, entries) in user_langs {
        langs.entry(lang).or_default().extend(entries);
    }
    let theme = Theme::from_all_overrides(&flat, &langs)
        .with_context(|| format!("invalid [theme] in {}", path.display()))?;
    Ok(Config {
        base: file.base,
        editor: file.editor.unwrap_or_else(|| EDITOR_DEFAULT.to_string()),
        keymap,
        theme,
    })
}

/// Colorschemes drift ships with. `onedark` is the base palette itself,
/// so it contributes no overrides.
pub const BUILTIN_COLORSCHEMES: &[&str] = &["onedark"];

/// Resolve a colorscheme name to theme overrides: a built-in, or a
/// `themes/<name>.toml` file next to the config (same shape as the
/// `[theme]` section: color entries plus per-language tables).
fn resolve_colorscheme(
    name: &str,
    config_dir: Option<&Path>,
) -> Result<(HashMap<String, String>, LangThemes)> {
    if BUILTIN_COLORSCHEMES.contains(&name) {
        return Ok((HashMap::new(), HashMap::new()));
    }
    let file = config_dir.map(|dir| dir.join("themes").join(format!("{name}.toml")));
    let Some(file) = file.filter(|f| f.exists()) else {
        bail!(
            "unknown colorscheme '{name}' (built-in: {}; user themes live in {}themes/{name}.toml)",
            BUILTIN_COLORSCHEMES.join(", "),
            config_dir.map_or(String::new(), |d| format!("{}/", d.display())),
        );
    };
    let raw: HashMap<String, toml::Value> = toml::from_str(
        &std::fs::read_to_string(&file)
            .with_context(|| format!("cannot read {}", file.display()))?,
    )
    .with_context(|| format!("invalid theme file {}", file.display()))?;
    split_theme(&raw).with_context(|| format!("invalid theme file {}", file.display()))
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
         # Base color theme. Built-in: onedark. A custom name loads\n\
         # themes/<name>.toml from this directory — same keys as [theme]\n\
         # below, with per-language sections like [typescript]; missing\n\
         # keys keep the built-in defaults. [theme] overrides win on top.\n\
         # colorscheme = \"onedark\"\n\n\
         # Editor for the open-in-editor key. {file} and {line} are\n\
         # substituted; the file path is appended when {file} is absent.\n\
         #   editor = \"code -g {file}:{line}\"   (Windows: \"code.cmd\")\n\
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
    out.push_str(
        "\n# Per-language overrides of the syntax palette: any [theme.<lang>]\n\
         # section (rust, python, javascript, typescript, tsx, go) may reset\n\
         # any syntax key for that language only.\n",
    );
    let mut last_lang = "";
    for (lang, key, value) in THEME_LANG_DEFAULTS {
        if *lang != last_lang {
            out.push_str(&format!("[theme.{lang}]\n"));
            last_lang = lang;
        }
        out.push_str(&format!("{key} = \"{value}\"\n"));
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
        let (flat, langs) = split_theme(&file.theme).unwrap();
        // The generated file carries the per-language defaults explicitly.
        assert!(langs.contains_key("go"));
        assert!(Theme::from_all_overrides(&flat, &langs).is_ok());
    }

    #[test]
    fn colorscheme_file_layers_under_theme_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("themes")).unwrap();
        std::fs::write(
            dir.path().join("themes/mytheme.toml"),
            "keyword = \"#111111\"\nstring = \"#222222\"\n\n[rust]\nbracket = \"#333333\"\n",
        )
        .unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "colorscheme = \"mytheme\"\n\n[theme]\nstring = \"#444444\"\n",
        )
        .unwrap();
        let config = load_at(&config_path).unwrap();
        // From the theme file…
        assert_eq!(
            config.theme.keyword,
            ratatui::style::Color::Rgb(0x11, 0x11, 0x11)
        );
        // …user [theme] override wins over the theme file…
        assert_eq!(
            config.theme.string,
            ratatui::style::Color::Rgb(0x44, 0x44, 0x44)
        );
        // …its language section applies…
        let rust = config.theme.for_lang("rust").unwrap();
        assert_eq!(
            rust["bracket"],
            ratatui::style::Color::Rgb(0x33, 0x33, 0x33)
        );
        // …and unset keys keep the built-in default.
        assert_eq!(
            config.theme.function,
            ratatui::style::Color::Rgb(0x61, 0xaf, 0xef)
        );
    }

    #[test]
    fn unknown_colorscheme_errors_helpfully() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "colorscheme = \"nope\"\n").unwrap();
        let err = load_at(&config_path).err().expect("must fail").to_string();
        assert!(err.contains("unknown colorscheme 'nope'"), "{err}");
        assert!(err.contains("onedark"), "{err}");
    }

    #[test]
    fn theme_rejects_unknown_language_and_non_syntax_keys() {
        let bad_lang: HashMap<String, toml::Value> =
            toml::from_str("cobol = { bracket = \"#ffffff\" }").unwrap();
        let (flat, langs) = split_theme(&bad_lang).unwrap();
        assert!(Theme::from_all_overrides(&flat, &langs).is_err());

        let bad_key: HashMap<String, toml::Value> =
            toml::from_str("rust = { cursor_bg = \"#ffffff\" }").unwrap();
        let (flat, langs) = split_theme(&bad_key).unwrap();
        assert!(Theme::from_all_overrides(&flat, &langs).is_err());
    }
}
