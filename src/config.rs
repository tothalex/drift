//! Configuration: `~/.config/drift/config.toml` (or `$XDG_CONFIG_HOME`),
//! with `[keys]` and `[theme]` sections. Missing file → defaults; invalid
//! file → a startup error naming what's wrong. `--init-config` writes the
//! full default file to edit from.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::connect::{AgentConfig, Backend, TEMPLATE_DEFAULT};
use crate::forge::ForgeConfig;
use crate::keymap::{KEY_DEFAULTS, Keymap};
use crate::theme::{THEME_DEFAULTS, THEME_LANG_DEFAULTS, Theme};
use crate::update::UpdateConfig;

/// Default editor command: `{line}` and `{file}` are substituted; the
/// file path is appended when `{file}` doesn't appear.
pub const EDITOR_DEFAULT: &str = "nvim +{line}";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    /// Default base branch; the `--base` flag overrides it, and without
    /// either the base is auto-detected (origin/HEAD, main, master;
    /// an orphan branch falls back to its upstream).
    #[serde(default)]
    base: Option<String>,
    /// Base color theme: a built-in name or a file in `themes/` next to
    /// the config. `[theme]` entries override on top of it.
    #[serde(default)]
    colorscheme: Option<String>,
    /// Editor command for the open-in-editor action.
    #[serde(default)]
    editor: Option<String>,
    /// Nerd Font file icons in the tree. Opt-in: a terminal never
    /// reports its font, so a missing patched font can't be detected.
    #[serde(default)]
    icons: Option<bool>,
    /// Pull-request integration; see [`ForgeSection`].
    #[serde(default)]
    forge: ForgeSection,
    /// Send-to-agent integration; see [`AgentSection`].
    #[serde(default)]
    agent: AgentSection,
    /// The new-release launch check; see [`UpdateSection`].
    #[serde(default)]
    update: UpdateSection,
    #[serde(default)]
    keys: HashMap<String, Vec<String>>,
    /// Flat color entries plus `[theme.<lang>]` per-language sub-tables,
    /// split apart in [`load`].
    #[serde(default)]
    theme: HashMap<String, toml::Value>,
}

/// The `[forge]` section: pull-request integration via the gh/glab CLIs.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgeSection {
    /// "github" | "gitlab" — overrides host detection for self-hosted
    /// instances whose hostname names neither.
    #[serde(default)]
    kind: Option<String>,
    /// Binary path overrides.
    #[serde(default)]
    gh: Option<String>,
    #[serde(default)]
    glab: Option<String>,
    /// Mirror the review checks (`x`) onto the pull request's per-file
    /// "viewed" ticks. GitHub only; GitLab exposes no such API.
    #[serde(default)]
    viewed_sync: Option<bool>,
}

impl ForgeSection {
    fn into_config(self) -> Result<ForgeConfig> {
        if let Some(kind) = self.kind.as_deref()
            && !matches!(kind, "github" | "gitlab")
        {
            bail!("forge.kind must be \"github\" or \"gitlab\", not '{kind}'");
        }
        Ok(ForgeConfig {
            kind: self.kind,
            gh: self.gh,
            glab: self.glab,
        })
    }
}

/// The `[agent]` section: sending code to an AI agent pane through the
/// surrounding multiplexer.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentSection {
    /// "auto" | "herdr" | "tmux" | "cmux" | "off" — auto uses the
    /// session drift runs inside; naming one forces it from outside.
    #[serde(default)]
    backend: Option<String>,
    /// Pin the target: an agent name ("claude") or pane id.
    #[serde(default)]
    target: Option<String>,
    /// Press enter in the agent pane after inserting the prompt.
    #[serde(default)]
    submit: Option<bool>,
    /// Prompt template; {input}, {file} (absolute), {relfile}, {lines},
    /// {start}, {end} and {code} are substituted.
    #[serde(default)]
    template: Option<String>,
}

impl AgentSection {
    fn into_config(self) -> Result<AgentConfig> {
        Ok(AgentConfig {
            // The known names live with the backend registry, so a new
            // backend never touches this file.
            backend: Backend::parse(self.backend.as_deref())?,
            target: self.target.filter(|t| !t.is_empty()),
            submit: self.submit.unwrap_or(true),
            template: self
                .template
                .unwrap_or_else(|| TEMPLATE_DEFAULT.to_string()),
        })
    }
}

/// The `[update]` section: whether launch checks for a newer release
/// (at most one network request a day) and mentions it in the status bar.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateSection {
    #[serde(default)]
    check: Option<bool>,
}

impl UpdateSection {
    fn into_config(self) -> UpdateConfig {
        UpdateConfig {
            check: self.check.unwrap_or(true),
        }
    }
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
    /// Nerd Font file icons in the tree.
    pub icons: bool,
    pub forge: ForgeConfig,
    /// Mirror review checks onto the forge's per-file viewed state.
    pub viewed_sync: bool,
    pub agent: AgentConfig,
    pub update: UpdateConfig,
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
        // A missing config means defaults; any other read failure
        // (permissions, I/O) is the one thing the user can't diagnose
        // from silently-default behavior — surface it.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => ConfigFile::default(),
        Err(err) => {
            return Err(err).with_context(|| format!("cannot read {}", path.display()));
        }
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
    let viewed_sync = file.forge.viewed_sync.unwrap_or(true);
    let forge = file
        .forge
        .into_config()
        .with_context(|| format!("invalid [forge] in {}", path.display()))?;
    let agent = file
        .agent
        .into_config()
        .with_context(|| format!("invalid [agent] in {}", path.display()))?;
    Ok(Config {
        base: file.base,
        editor: file.editor.unwrap_or_else(|| EDITOR_DEFAULT.to_string()),
        icons: file.icons.unwrap_or(false),
        forge,
        viewed_sync,
        agent,
        update: file.update.into_config(),
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
         editor = \"nvim +{line}\"\n\n\
         # Nerd Font file icons in the tree. Opt-in: needs a patched\n\
         # font (nerdfonts.com) in the terminal, which drift cannot\n\
         # detect. `drift --icons` turns it on for one session.\n\
         # icons = false\n\n\
         # Pull-request view (the p key) talks to GitHub/GitLab through\n\
         # the official gh/glab CLIs — install one and run its `auth\n\
         # login`. The forge is detected from the origin remote URL; set\n\
         # kind for self-hosted hosts naming neither \"github\" nor\n\
         # \"gitlab\".\n\
         # In a pull request the x key also ticks the file off as\n\
         # \"viewed\" on GitHub, and the ticks already there come back\n\
         # as checks when you open it; set viewed_sync = false (or pass\n\
         # --no-viewed-sync) to keep the checks to yourself. GitLab has\n\
         # no such API, so there the checks are always local.\n\
         # [forge]\n\
         # kind = \"github\"          # or \"gitlab\"\n\
         # gh = \"/path/to/gh\"       # binary overrides\n\
         # glab = \"/path/to/glab\"\n\
         # viewed_sync = true       # mirror x onto GitHub's Viewed\n\n\
         # Send the current line or visual selection to an AI agent pane\n\
         # (the s key): the prompt you type lands in a running claude/\n\
         # codex/… CLI in a sibling pane of the herdr, tmux or cmux\n\
         # session drift runs inside; naming a backend forces it from\n\
         # any terminal (cmux answers only its own surfaces). In the\n\
         # template, {input} is the typed\n\
         # prompt; {file} (absolute path), {relfile} (repo-relative),\n\
         # {lines}, {start}, {end} and {code} describe the selection.\n\
         # [agent]\n\
         # backend = \"auto\"         # auto | herdr | tmux | cmux | off\n\
         # target = \"\"              # pin an agent name or pane id\n\
         # submit = true            # press enter after inserting\n\
         # template = \"{input}\\n\\n{file}:{lines}\\n```\\n{code}\\n```\"\n\n\
         # Launch checks GitHub for a newer release (at most once a day)\n\
         # and mentions it in the status bar; `drift update` installs it.\n\
         # [update]\n\
         # check = true\n\n[keys]\n",
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
         # section naming an installed or curated language (rust, typescript,\n\
         # go, … — see `drift lang list`) may reset any syntax key for that\n\
         # language only.\n",
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
    fn agent_section_parses_and_validates_backend() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[agent]\nbackend = \"herdr\"\ntarget = \"claude\"\nsubmit = false\n",
        )
        .unwrap();
        let config = load_at(&config_path).unwrap();
        assert_eq!(config.agent.backend, Backend::Herdr);
        assert_eq!(config.agent.target.as_deref(), Some("claude"));
        assert!(!config.agent.submit);
        assert_eq!(config.agent.template, TEMPLATE_DEFAULT);

        // Defaults without the section: auto, submit, no pin.
        std::fs::write(&config_path, "").unwrap();
        let config = load_at(&config_path).unwrap();
        assert_eq!(config.agent.backend, Backend::Auto);
        assert!(config.agent.submit);
        assert!(config.agent.target.is_none());

        std::fs::write(&config_path, "[agent]\nbackend = \"zellij\"\n").unwrap();
        let err = load_at(&config_path).err().expect("must fail").to_string();
        assert!(err.contains("invalid [agent]"), "{err}");
    }

    #[test]
    fn update_section_parses_and_defaults_on() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "[update]\ncheck = false\n").unwrap();
        assert!(!load_at(&config_path).unwrap().update.check);

        std::fs::write(&config_path, "").unwrap();
        assert!(load_at(&config_path).unwrap().update.check);
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
