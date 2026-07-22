//! The app coordinator: session state (VCS, files, current file), the
//! event loop, and dispatch into the focused sub-states.
//!
//! Navigation is a light two-pane focus: `h`/`l` (or clicking) aim the
//! cursor keys at the file tree or the code view. Everything else stays
//! global — no modes, no prefixes.

pub mod code_view;
pub mod compose;
pub mod panes;
pub mod picker;
pub mod pr;
pub mod review;
pub mod tree_nav;
pub mod view_cache;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;
use ratatui::layout::Position;

use crate::config::Config;
use crate::events::{
    AppEvent, INPUT_POLL_MS, RefreshedComments, pop_keyboard_enhancement,
    push_keyboard_enhancement, spawn_input_thread, spawn_watcher_thread,
};
use crate::forge::model::{Anchor, ComposeTarget, PrData, PrDetail, PullRequest, Side};
use crate::forge::{self, Forge, ForgeConfig, ForgeError};
use crate::keymap::{Action, Keymap};
use crate::processor::view::{FileView, FlatLine, ViewLine, char_to_byte};
use crate::theme::Theme;
use crate::ui::CODE_GUTTER;
use crate::vcs::model::{ChangedFile, Comparison, LineKind, Scope};
use crate::vcs::{self, Vcs};

use code_view::{CodeView, TextPos};
use compose::Compose;
use panes::PaneLayout;
use picker::pr_label;
pub use picker::{BasePicker, Picker, PrPicker, ScopePicker};
use pr::PrSession;
use review::Review;
use tree_nav::TreeNav;
use view_cache::ViewCache;

/// Which pane the cursor keys act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Tree,
    Code,
}

/// Live-reload bookkeeping: paths the watcher flagged since the last
/// applied refresh (their views are stale), whether git metadata moved
/// (the comparison itself may be stale), and the background status-scan
/// sequencing — results carry the sequence they were started with,
/// stale ones are dropped.
#[derive(Default)]
struct ScanState {
    dirty_paths: HashSet<PathBuf>,
    meta_pending: bool,
    seq: u64,
    inflight: bool,
    /// Events arrived while a scan was running: go again when it lands.
    rescan_needed: bool,
}

impl ScanState {
    /// A manual reload/refresh supersedes any in-flight live refresh.
    fn cancel(&mut self) {
        self.seq += 1;
        self.inflight = false;
        self.rescan_needed = false;
        self.meta_pending = false;
        self.dirty_paths.clear();
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

/// One in-flight forge request: everything about the wait, cleared as a
/// unit when its response (matched by `seq`) lands.
struct ForgeRequest {
    seq: u64,
    /// Where in the code view the mutation acts — the spinner renders on
    /// that row, where the user is looking. `None` for list/load waits.
    spot: Option<ActionSpot>,
    /// Status-bar text for when the request lands ("comment posted", …).
    done_notice: String,
}

/// The code-view row an in-flight forge mutation targets; the renderer
/// puts the waiting spinner right there.
#[derive(Clone)]
pub enum ActionSpot {
    /// The diff line a new inline comment goes under.
    DiffLine {
        path: PathBuf,
        old: Option<u32>,
        new: Option<u32>,
    },
    /// A thread's hint row: replies land there, resolution covers it.
    ThreadHint { key: String },
    /// One comment's author row (deletion).
    CommentHead { id: String },
    /// The conversation's own hint row (general comments).
    ConversationHint,
}

pub struct App {
    vcs: Box<dyn Vcs>,
    pub cmp: Comparison,
    pub keymap: Keymap,
    pub theme: Theme,
    pub files: Vec<ChangedFile>,
    pub nav: TreeNav,
    pub code: CodeView,
    pub layout: PaneLayout,
    pub cache: ViewCache,
    review: Review,
    /// The file whose diff is shown — stays put while the cursor is on a
    /// directory row.
    current: Option<usize>,
    /// The pane the cursor keys act on (`h`/`l` or a click switch it).
    focus: Pane,
    /// The `?` keybinding overlay is open; any key closes it.
    help_open: bool,
    /// The open picker overlay (base branch or review scope), if any.
    picker: Option<Picker>,
    /// Vim-style count prefix: typed digits repeat the next motion.
    count: Option<usize>,
    /// Search: one independent query per pane (highlights persist until
    /// Esc or `q`)…
    search_tree: String,
    search_code: String,
    /// …and whether `/` input mode is capturing keystrokes — it edits
    /// the focused pane's query; `n`/`N` always follow the focus.
    search_input: bool,
    /// One-shot status message ("yanked 3 lines"); cleared on next key.
    notice: Option<String>,
    /// Event channel shared by the input thread and prefetch workers.
    events_tx: Sender<AppEvent>,
    events_rx: Receiver<AppEvent>,
    /// Bumped whenever cached views become stale; prefetch results from
    /// older generations are discarded on arrival.
    generation: u64,
    /// Live-reload bookkeeping: what the watcher flagged and where the
    /// background status scan stands.
    scan: ScanState,
    /// Forge integration: the `[forge]` config, the lazily-detected forge
    /// (shared with background fetch threads), and the staleness counter
    /// for forge results (stale sequences are dropped on arrival).
    forge_config: ForgeConfig,
    forge: Option<Arc<dyn Forge>>,
    forge_seq: u64,
    /// The forge request being waited on, if any: its staleness sequence,
    /// where in the code view it acts, and the success notice. One value,
    /// so nothing clears half of it. The spinner animates while set,
    /// driven by a ticker thread that only lives that long
    /// (`spinner_running` is the thread's own kill switch).
    forge_request: Option<ForgeRequest>,
    spinner: usize,
    spinner_running: Arc<AtomicBool>,
    /// The open pull-request session, if any; while set, files and views
    /// come from the forge data instead of the working tree.
    pr: Option<PrSession>,
    /// Editor command template ({file}/{line} placeholders).
    editor: String,
    /// While set, the input thread stops reading the terminal — an
    /// external editor owns it.
    input_paused: Arc<AtomicBool>,
    /// An editor launch requested by the last key, performed by the run
    /// loop (it needs the terminal handle).
    pending_editor: Option<(PathBuf, u32)>,
    /// The in-app comment composer overlay, if open. It captures all
    /// keystrokes while set (like search input).
    compose: Option<Compose>,
    /// Deleting a comment takes `d` twice: the id armed by the first
    /// press. Any other key disarms.
    pending_delete: Option<String>,
    /// The terminal disambiguates modified keys (kitty protocol), so
    /// shift+enter is distinguishable from enter in the composer.
    keyboard_enhanced: bool,
    quit: bool,
}

impl App {
    pub fn new(vcs: Box<dyn Vcs>, base_override: Option<&str>, config: Config) -> Result<App> {
        let cmp = vcs.comparison(base_override)?;
        let (events_tx, events_rx) = channel();
        let mut app = App {
            vcs,
            cmp,
            keymap: config.keymap,
            theme: config.theme,
            files: Vec::new(),
            nav: TreeNav::new(&[]),
            code: CodeView::new(),
            layout: PaneLayout::new(),
            cache: ViewCache::new(),
            review: Review::default(),
            current: None,
            focus: Pane::Tree,
            help_open: false,
            picker: None,
            count: None,
            search_tree: String::new(),
            search_code: String::new(),
            search_input: false,
            notice: None,
            events_tx,
            events_rx,
            generation: 0,
            scan: ScanState::default(),
            forge_config: config.forge,
            forge: None,
            forge_seq: 0,
            forge_request: None,
            spinner: 0,
            spinner_running: Arc::new(AtomicBool::new(false)),
            pr: None,
            editor: config.editor,
            input_paused: Arc::new(AtomicBool::new(false)),
            pending_editor: None,
            compose: None,
            pending_delete: None,
            keyboard_enhanced: false,
            quit: false,
        };
        app.reload()?;
        Ok(app)
    }

    // --- accessors for the UI ---

    pub fn current_file(&self) -> Option<&ChangedFile> {
        self.current.and_then(|i| self.files.get(i))
    }

    pub fn current_view(&self) -> Option<&FileView> {
        self.current
            .and_then(|i| self.files.get(i))
            .and_then(|f| self.cache.get(&f.path))
    }

    pub fn help_open(&self) -> bool {
        self.help_open
    }

    /// The pane the cursor keys act on; its header lights up.
    pub fn focused_pane(&self) -> Pane {
        self.focus
    }

    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    /// The open pull-request session, if any.
    pub fn pr_session(&self) -> Option<&PrSession> {
        self.pr.as_ref()
    }

    /// The open comment composer, if any.
    pub fn compose(&self) -> Option<&Compose> {
        self.compose.as_ref()
    }

    /// Whether shift+enter is distinguishable from enter — decides which
    /// newline key the composer's footer advertises.
    pub fn keyboard_enhanced(&self) -> bool {
        self.keyboard_enhanced
    }

    /// The spinner frame to show while a forge request is in flight.
    pub fn spinner_frame(&self) -> Option<&'static str> {
        const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        self.forge_request
            .is_some()
            .then(|| FRAMES[self.spinner % FRAMES.len()])
    }

    /// The code-view row the in-flight mutation targets, with the current
    /// spinner frame — the diff renderer marks that row.
    pub fn action_spot(&self) -> Option<(&ActionSpot, &'static str)> {
        Some((
            self.forge_request.as_ref()?.spot.as_ref()?,
            self.spinner_frame()?,
        ))
    }

    /// A forge request just started: make sure a ticker thread is
    /// animating the spinner (one at a time; it exits by itself once
    /// nothing is in flight).
    fn start_spinner(&mut self) {
        if self.spinner_running.swap(true, Ordering::Relaxed) {
            return; // already ticking
        }
        let running = Arc::clone(&self.spinner_running);
        let tx = self.events_tx.clone();
        std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                if tx.send(AppEvent::Tick).is_err() {
                    return; // app is gone
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }

    /// Is this file index the PR session's virtual conversation entry?
    pub fn is_pr_conversation(&self, index: usize) -> bool {
        self.pr.is_some() && self.files.get(index).is_some_and(pr::is_conversation)
    }

    /// Tree label for the conversation entry, with its comment count.
    pub fn pr_conversation_label(&self) -> String {
        let count = self.pr.as_ref().map_or(0, |session| {
            session.data.conversation.len()
                + session
                    .data
                    .threads
                    .iter()
                    .filter(|thread| thread.anchor.is_none())
                    .count()
        });
        match count {
            0 => "conversation".to_string(),
            count => format!("conversation ({count})"),
        }
    }

    /// The status bar's comparison segment, e.g. " main ← feature " — or
    /// the open pull request. Also the click target that opens the base
    /// picker (or, in a session, the PR picker).
    pub fn comparison_label(&self) -> String {
        if let Some(session) = &self.pr {
            let detail = &session.data.detail;
            let mut title: String = detail.title.chars().take(40).collect();
            if title.len() < detail.title.len() {
                title.push('…');
            }
            return format!(" #{} {} ", detail.number, title);
        }
        let scope = match &self.cmp.scope {
            Scope::All => String::new(),
            Scope::Untracked => " · untracked".to_string(),
            Scope::Commit(rev) => format!(" · {}", &rev.0[..rev.0.len().min(7)]),
        };
        format!(
            " {} ← {}{} ",
            self.cmp.base_label, self.cmp.work_label, scope
        )
    }

    pub fn pending_count(&self) -> Option<usize> {
        self.count
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// The focused pane's search query (what `/` edits and `n` follows).
    pub fn search_query(&self) -> &str {
        match self.focus {
            Pane::Tree => &self.search_tree,
            Pane::Code => &self.search_code,
        }
    }

    fn search_query_mut(&mut self) -> &mut String {
        match self.focus {
            Pane::Tree => &mut self.search_tree,
            Pane::Code => &mut self.search_code,
        }
    }

    /// The tree's own query — highlighted in the file list.
    pub fn tree_search(&self) -> &str {
        &self.search_tree
    }

    /// The code view's own query — highlighted in the diff.
    pub fn code_search(&self) -> &str {
        &self.search_code
    }

    pub fn search_input(&self) -> bool {
        self.search_input
    }

    pub fn is_checked(&self, file_index: usize) -> bool {
        self.files
            .get(file_index)
            .is_some_and(|f| self.review.contains(&f.path))
    }

    pub fn checked_count(&self) -> usize {
        self.review.count_in(&self.files)
    }

    /// Rows of the current view's flattening — the code cursor's range.
    fn view_len(&self) -> usize {
        self.current_view().map_or(0, FileView::flat_len)
    }

    /// Visible rows of the tree / code panes, for cursor clamping.
    fn tree_viewport(&self) -> usize {
        self.layout.tree_area.height as usize
    }

    fn code_viewport(&self) -> usize {
        self.layout.code_area.height as usize
    }

    /// The flattened view row under the code cursor.
    fn line_at_cursor(&self) -> Option<FlatLine<'_>> {
        self.current_view()?.flat_lines().nth(self.code.cursor)
    }

    /// UI copy: "pull request" or "merge request", once a forge is known.
    fn noun(&self) -> &'static str {
        self.forge
            .as_ref()
            .map_or("pull request", |forge| forge.pr_noun())
    }

    /// Where the reviewer is, for restoring after the file list moves:
    /// the shown file's path and the new-side line under the cursor.
    fn current_anchor(&self) -> (Option<PathBuf>, Option<u32>) {
        let path = self.current_file().map(|f| f.path.clone());
        let lineno = self
            .current_view()
            .and_then(|view| view.lineno_at(self.code.cursor));
        (path, lineno)
    }

    /// Called by the renderer once geometry is known.
    pub fn apply_pending_center(&mut self) {
        let viewport = self.code_viewport();
        let len = self.view_len();
        self.code.apply_pending_center(viewport, len);
    }

    /// `--pr`: queue a pull-request load before the event loop starts;
    /// the result is picked up on the first turns of [`Self::run`].
    pub fn open_pr_at_start(&mut self, number: u64) {
        self.open_pr(number);
    }

    // --- event loop ---

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.keyboard_enhanced = push_keyboard_enhancement();
        spawn_input_thread(self.events_tx.clone(), Arc::clone(&self.input_paused));
        spawn_watcher_thread(self.events_tx.clone(), self.vcs.root().to_path_buf());
        while !self.quit {
            terminal.draw(|frame| crate::ui::draw(frame, self))?;
            let result = match self.events_rx.recv()? {
                AppEvent::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    self.handle_key(key)
                }
                AppEvent::Input(Event::Mouse(mouse)) => self.handle_mouse(mouse),
                AppEvent::Input(_) => Ok(()), // resize etc. — redraw on next loop
                AppEvent::ViewReady {
                    generation,
                    index,
                    view,
                } => {
                    // The generation guard also means `index` still refers
                    // to the files list the prefetch was started with.
                    if generation == self.generation
                        && let Some(file) = self.files.get(index)
                    {
                        self.cache.insert_if_absent(file.path.clone(), view);
                    }
                    Ok(())
                }
                AppEvent::FsChanged { paths, meta } => {
                    self.on_fs_changed(paths, meta);
                    Ok(())
                }
                AppEvent::StatusReady { seq, result } => self.on_status_ready(seq, result),
                AppEvent::PrListReady { seq, result } => {
                    self.on_pr_list_ready(seq, result);
                    Ok(())
                }
                AppEvent::PrReady { seq, result } => self.on_pr_ready(seq, result),
                AppEvent::PrPosted { seq, result } => self.on_pr_posted(seq, result),
                AppEvent::Tick => {
                    if self.forge_request.is_some() {
                        self.spinner = self.spinner.wrapping_add(1);
                    } else {
                        // The wait ended: let the ticker thread die.
                        self.spinner_running.store(false, Ordering::Relaxed);
                    }
                    Ok(())
                }
            };
            // After startup, failures (e.g. git during a rebase) surface in
            // the status bar instead of exiting the app.
            if let Err(err) = result {
                self.notice = Some(format!("error: {err:#}"));
            }
            // Editor launches run here, not in the key handler: the
            // terminal must be handed over and re-initialized around them.
            if let Some((path, line)) = self.pending_editor.take() {
                self.open_editor(terminal, &path, line);
            }
        }
        if self.keyboard_enhanced {
            pop_keyboard_enhancement();
        }
        Ok(())
    }

    /// Suspend the TUI, run the editor on the file, and restore. The
    /// input thread is paused for the duration so the editor gets every
    /// keystroke; any resulting file change comes back via live reload.
    fn open_editor(&mut self, terminal: &mut DefaultTerminal, path: &Path, line: u32) {
        let Some(mut command) = editor_command(&self.editor, path, line) else {
            self.notice = Some(format!("invalid editor command '{}'", self.editor));
            return;
        };
        self.input_paused.store(true, Ordering::Relaxed);
        // The input thread notices the pause within one poll interval;
        // wait that out so it can't race the editor for the terminal.
        std::thread::sleep(Duration::from_millis(INPUT_POLL_MS + 20));
        if self.keyboard_enhanced {
            pop_keyboard_enhancement();
        }
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        let status = command.status();
        *terminal = ratatui::init();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
        if self.keyboard_enhanced {
            self.keyboard_enhanced = push_keyboard_enhancement();
        }
        self.input_paused.store(false, Ordering::Relaxed);
        if let Err(err) = status {
            let program = command.get_program().to_string_lossy().into_owned();
            self.notice = Some(format!("cannot run '{program}': {err}"));
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.help_open {
            self.help_open = false;
            self.count = None;
            return Ok(());
        }
        if self.picker.is_some() {
            return self.handle_picker_key(key.code);
        }
        if self.compose.is_some() {
            self.handle_compose_key(key);
            return Ok(());
        }
        // `/` input mode captures keystrokes; the cursor follows matches
        // live, yazi-style. Enter keeps the matches, Esc cancels.
        if self.search_input {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.quit = true;
                }
                KeyCode::Esc => {
                    self.search_input = false;
                    self.search_query_mut().clear();
                }
                KeyCode::Enter => self.search_input = false,
                KeyCode::Backspace => {
                    self.search_query_mut().pop();
                    self.jump_to_first_match()?;
                }
                KeyCode::Char(c) => {
                    self.search_query_mut().push(c);
                    self.jump_to_first_match()?;
                }
                _ => {}
            }
            return Ok(());
        }
        // Vim-style count prefix: digits accumulate and repeat the next
        // motion (`10j`). A bare `0` only counts once a prefix started.
        if let KeyCode::Char(digit @ '0'..='9') = key.code
            && !(digit == '0' && self.count.is_none())
        {
            let digit = digit as usize - '0' as usize;
            self.count = Some((self.count.unwrap_or(0) * 10 + digit).min(9999));
            return Ok(());
        }
        self.notice = None;
        // Any non-digit key consumes the count; motions repeat by it.
        let explicit_count = self.count.is_some();
        let count = self.count.take().unwrap_or(1).max(1) as isize;
        // Esc only cancels things (visual mode, search highlights); it
        // never quits.
        if key.code == KeyCode::Esc {
            self.code.select_anchor = None;
            self.search_tree.clear();
            self.search_code.clear();
            return Ok(());
        }
        // Comment rows carry a fixed micro-map (like the picker): the
        // keys their hint rows advertise act on the comment or thread
        // under the cursor, shadowing the global bindings there. A
        // pending delete survives exactly one keypress.
        let pending_delete = self.pending_delete.take();
        if self.pr.is_some()
            && self.focus == Pane::Code
            && self.handle_comment_key(key.code, pending_delete)
        {
            return Ok(());
        }
        let Some(action) = self.keymap.action_for(key.code, key.modifiers) else {
            return Ok(());
        };
        match action {
            // In visual mode `q` leaves the mode, like Esc — it must not
            // quit the app mid-selection.
            Action::Quit if self.code.select_anchor.is_some() => {
                self.code.select_anchor = None;
            }
            // Likewise with search highlights active: `q` steps back to
            // normal instead of quitting.
            Action::Quit if !self.search_tree.is_empty() || !self.search_code.is_empty() => {
                self.search_tree.clear();
                self.search_code.clear();
            }
            Action::Quit => self.quit = true,
            Action::Help => self.help_open = true,
            Action::FocusTree => self.focus = Pane::Tree,
            Action::FocusCode => self.focus = Pane::Code,
            Action::NextFile => self.move_file(count)?,
            Action::PrevFile => self.move_file(-count)?,
            Action::ToggleDir => self.nav.toggle_dir(),
            Action::CursorDown => self.move_focused(count)?,
            Action::CursorUp => self.move_focused(-count)?,
            Action::JumpDown => self.move_focused(15 * count)?,
            Action::JumpUp => self.move_focused(-15 * count)?,
            Action::JumpTop => {
                let target = if explicit_count {
                    count as usize - 1
                } else {
                    0
                };
                self.jump_focused(target)?;
            }
            Action::JumpBottom => {
                let target = if explicit_count {
                    count as usize - 1
                } else {
                    usize::MAX
                };
                self.jump_focused(target)?;
            }
            Action::ScopeWiden => self.adjust_scope(count)?,
            Action::ScopeNarrow => self.adjust_scope(-count)?,
            Action::Search => {
                self.search_query_mut().clear();
                self.search_input = true;
            }
            Action::NextMatch => self.jump_match(1)?,
            Action::PrevMatch => self.jump_match(-1)?,
            Action::ToggleCollapse => {
                self.cache.expand_unchanged = !self.cache.expand_unchanged;
                self.reload_current_view()?;
            }
            Action::ToggleCommentFold => {
                self.cache.comment_fold = !self.cache.comment_fold;
                self.reload_current_view()?;
            }
            Action::ToggleThread => self.toggle_thread()?,
            Action::CheckFile => self.toggle_check()?,
            Action::UncheckLast => self.uncheck_last()?,
            Action::Visual => self.code.toggle_visual(),
            Action::Yank => self.yank(),
            Action::CopyPath => self.copy_path(),
            Action::GrowTree => self.layout.resize(2 * count),
            Action::ShrinkTree => self.layout.resize(-2 * count),
            Action::PickBase if self.pr.is_some() => {
                self.notice = Some("in a pull request — p picks another or exits".to_string());
            }
            Action::PickBase => self.open_base_picker()?,
            Action::PickPr => self.open_pr_picker(),
            Action::Comment => self.request_comment(false),
            Action::CommentGeneral => self.request_comment(true),
            Action::Refresh => match self.pr.as_ref().map(|s| s.data.detail.number) {
                Some(number) => self.open_pr(number),
                None => self.refresh()?,
            },
            Action::OpenEditor => self.request_editor(),
        }
        Ok(())
    }

    /// Is the position on the status bar's comparison segment? The status
    /// bar is the single row below the main area.
    fn on_comparison_label(&self, position: Position) -> bool {
        let status_row = self.layout.main_area.y + self.layout.main_area.height;
        position.y == status_row && (position.x as usize) < self.comparison_label().chars().count()
    }

    /// Queue the shown file for the editor, at the cursor's line.
    fn request_editor(&mut self) {
        let Some(file) = self.current_file() else {
            self.notice = Some("no file to open".to_string());
            return;
        };
        if pr::is_conversation(file) {
            self.notice = Some("the conversation is not a file".to_string());
            return;
        }
        let path = self.vcs.root().join(&file.path);
        let line = self
            .current_view()
            .and_then(|view| view.lineno_at(self.code.cursor))
            .unwrap_or(1);
        self.pending_editor = Some((path, line));
    }

    /// Picker keys are a fixed modal micro-map: j/k/arrows move, Enter
    /// selects, Esc/q cancel. Choosing a branch chains into the scope
    /// picker; choosing a scope applies it.
    fn handle_picker_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.picker = None,
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(picker) = &mut self.picker {
                    picker.move_cursor(-1);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(picker) = &mut self.picker {
                    picker.move_cursor(1);
                }
            }
            KeyCode::Enter => match self.picker.take() {
                Some(Picker::Base(picker)) => {
                    let base = picker.branches[picker.cursor].clone();
                    if self.set_base(&base)? {
                        self.open_scope_picker();
                    }
                }
                Some(Picker::Scope(picker)) => {
                    let scope = picker.entries[picker.cursor].0.clone();
                    self.set_scope(scope)?;
                }
                Some(Picker::Pr(picker)) => {
                    if picker.back && picker.cursor == 0 {
                        self.leave_pr_session()?;
                    } else if let Some(item) =
                        picker.items.get(picker.cursor - usize::from(picker.back))
                    {
                        self.open_pr(item.number);
                    }
                }
                None => {}
            },
            _ => {}
        }
        Ok(())
    }

    fn open_base_picker(&mut self) -> Result<()> {
        let branches = self.vcs.branches()?;
        if branches.is_empty() {
            self.notice = Some("no branches to compare against".to_string());
            return Ok(());
        }
        let cursor = branches
            .iter()
            .position(|b| *b == self.cmp.base_label)
            .unwrap_or(0);
        self.picker = Some(Picker::Base(BasePicker { branches, cursor }));
        Ok(())
    }

    fn open_scope_picker(&mut self) {
        let commits = match self.vcs.commits(&self.cmp) {
            Ok(commits) => commits,
            Err(err) => {
                self.notice = Some(format!("cannot list commits: {err}"));
                Vec::new()
            }
        };
        let mut entries = vec![
            (Scope::All, "all changes".to_string()),
            (Scope::Untracked, "untracked files".to_string()),
        ];
        entries.extend(commits.into_iter().map(|commit| {
            let label = format!("{} {}", commit.short_id, commit.summary);
            (Scope::Commit(commit.id), label)
        }));
        let cursor = entries
            .iter()
            .position(|(scope, _)| *scope == self.cmp.scope)
            .unwrap_or(0);
        self.picker = Some(Picker::Scope(ScopePicker { entries, cursor }));
    }

    // --- pull-request integration ---

    /// The forge for this repo, detected once and shared with background
    /// threads. Detection is local (remote URL + config) — only the
    /// listing/loading calls that follow talk to the network.
    fn forge(&mut self) -> Result<Arc<dyn Forge>, String> {
        if let Some(forge) = &self.forge {
            return Ok(Arc::clone(forge));
        }
        let forge: Arc<dyn Forge> = forge::detect(self.vcs.root(), &self.forge_config)
            .map_err(|err| err.to_string())?
            .into();
        self.forge = Some(Arc::clone(&forge));
        Ok(forge)
    }

    /// Start a forge request: bump the staleness sequence, record the
    /// wait as one value, and animate the spinner. Returns what the
    /// background thread needs — or `None` with the error already in the
    /// status bar when no forge is available.
    fn begin_forge_request(
        &mut self,
        spot: Option<ActionSpot>,
        done_notice: &str,
    ) -> Option<(Arc<dyn Forge>, u64, Sender<AppEvent>)> {
        let forge = match self.forge() {
            Ok(forge) => forge,
            Err(err) => {
                self.notice = Some(err);
                return None;
            }
        };
        self.forge_seq += 1;
        self.forge_request = Some(ForgeRequest {
            seq: self.forge_seq,
            spot,
            done_notice: done_notice.to_string(),
        });
        self.start_spinner();
        Some((forge, self.forge_seq, self.events_tx.clone()))
    }

    /// List open pull requests off the main thread and open the picker
    /// when they arrive; errors (CLI missing, unauthenticated, unknown
    /// forge) land in the status bar.
    fn open_pr_picker(&mut self) {
        let Some((forge, seq, tx)) = self.begin_forge_request(None, "") else {
            return;
        };
        self.notice = Some(format!("loading {}s…", forge.pr_noun()));
        std::thread::spawn(move || {
            let result = forge.list_open().map_err(|err| err.to_string());
            let _ = tx.send(AppEvent::PrListReady { seq, result });
        });
    }

    fn on_pr_list_ready(&mut self, seq: u64, result: Result<Vec<PullRequest>, String>) {
        if self.forge_request.as_ref().is_none_or(|r| r.seq != seq) {
            return; // superseded by a newer forge request
        }
        self.forge_request = None;
        let noun = self.noun();
        match result {
            Ok(items) if items.is_empty() && self.pr.is_none() => {
                self.notice = Some(format!("no open {noun}s"));
            }
            Ok(items) => {
                self.notice = None;
                // In a session the picker doubles as the way out.
                let back = self.pr.is_some();
                let active = self.pr.as_ref().map(|s| s.data.detail.number);
                let mut rows: Vec<(String, bool)> = Vec::new();
                if back {
                    rows.push(("← back to local changes".to_string(), false));
                }
                rows.extend(
                    items
                        .iter()
                        .map(|item| (pr_label(item), Some(item.number) == active)),
                );
                let cursor = rows.iter().position(|(_, current)| *current).unwrap_or(0);
                self.picker = Some(Picker::Pr(PrPicker {
                    title: format!("open {noun}s"),
                    rows,
                    items,
                    back,
                    cursor,
                }));
            }
            Err(err) => self.notice = Some(err),
        }
    }

    /// Load one pull request — detail, diffs, comments — off the main
    /// thread; [`AppEvent::PrReady`] lands in [`Self::on_pr_ready`].
    fn open_pr(&mut self, number: u64) {
        let Some((forge, seq, tx)) = self.begin_forge_request(None, "") else {
            return;
        };
        self.notice = Some(format!("loading {} #{number}…", forge.pr_noun()));
        std::thread::spawn(move || {
            let result = forge
                .load(number)
                .map(Box::new)
                .map_err(|err| err.to_string());
            let _ = tx.send(AppEvent::PrReady { seq, result });
        });
    }

    fn on_pr_ready(&mut self, seq: u64, result: Result<Box<PrData>, String>) -> Result<()> {
        if self.forge_request.as_ref().is_none_or(|r| r.seq != seq) {
            return Ok(()); // superseded by a newer forge request
        }
        self.forge_request = None;
        match result {
            Ok(data) => {
                self.notice = None;
                self.enter_pr_session(Arc::new(*data))?;
            }
            Err(err) => self.notice = Some(err),
        }
        Ok(())
    }

    /// Swap the app onto the pull request: its files (with the virtual
    /// conversation entry first) replace the local change list until
    /// [`Self::leave_pr_session`]. Review checks are keyed by path and
    /// deliberately survive re-entering the same PR.
    fn enter_pr_session(&mut self, data: Arc<PrData>) -> Result<()> {
        // Reloading the PR already open (`r`) keeps fold state and the
        // reviewer's place; a different PR starts fresh.
        let same = self
            .pr
            .as_ref()
            .is_some_and(|s| s.data.detail.number == data.detail.number);
        let collapsed = match self.pr.take() {
            Some(session) if same => session.collapsed,
            _ => HashSet::new(),
        };
        // Anchors, taken before anything moves.
        let (current_path, lineno) = self.current_anchor();
        self.pr = Some(PrSession { data, collapsed });
        let session = self.pr.as_ref().expect("just set");
        let mut files = vec![pr::conversation_entry()];
        files.extend(session.data.files.iter().map(|f| f.changed.clone()));
        self.files = files;
        self.cache.reset();
        if !same {
            self.nav.rebuild(&self.files);
            self.current = None;
            self.code.reset_for_new_view();
            self.sync_current()?;
            self.start_prefetch();
            return Ok(());
        }
        // Same refresh contract as a local refresh: the tree keeps its
        // cursor and collapsed dirs, the shown file stays shown, and its
        // code cursor re-anchors by line number.
        self.nav
            .rebuild_preserving(&self.files, self.tree_viewport());
        self.restore_place(current_path, lineno)?;
        self.start_prefetch();
        Ok(())
    }

    /// Back to reviewing local changes. A full reload also applies any
    /// working-tree changes the watcher noted during the session.
    fn leave_pr_session(&mut self) -> Result<()> {
        if self.pr.take().is_none() {
            return Ok(());
        }
        self.notice = None;
        self.reload()
    }

    /// The review thread under the code cursor, if it sits on one of a
    /// thread's rows (heads and bodies carry their forge-side key).
    fn thread_key_at_cursor(&self) -> Option<String> {
        match self.line_at_cursor()? {
            FlatLine::Line(
                ViewLine::CommentHead { key, .. }
                | ViewLine::CommentBody { key, .. }
                | ViewLine::CommentHint { key, .. },
            ) if !key.is_empty() => Some(key.clone()),
            _ => None,
        }
    }

    /// Fold or unfold the review thread under the cursor and recompute
    /// just this file's view — the data is local, nothing refetches.
    fn toggle_thread(&mut self) -> Result<()> {
        if self.pr.is_none() {
            return Ok(());
        }
        let Some(key) = self.thread_key_at_cursor() else {
            self.notice = Some("no review thread under the cursor".to_string());
            return Ok(());
        };
        let path = self.current_file().map(|file| file.path.clone());
        if let Some(session) = &mut self.pr
            && !session.collapsed.remove(&key)
        {
            session.collapsed.insert(key);
        }
        if let Some(path) = path {
            self.cache.remove(&path);
        }
        if let Some(index) = self.current {
            self.ensure_view(index)?;
        }
        let last = self.view_len().saturating_sub(1);
        self.code.cursor = self.code.cursor.min(last);
        Ok(())
    }

    /// The single comment under the cursor, as (comment id, is an inline
    /// review comment). Conversation comments have no thread key. Only a
    /// comment's own rows qualify — deletion is destructive, so the
    /// target must be exactly what the cursor is on (the hint row is
    /// thread-scoped and refuses, pointing at the comment instead).
    fn comment_at_cursor(&self) -> Option<(String, bool)> {
        match self.line_at_cursor()? {
            FlatLine::Line(
                ViewLine::CommentHead { id, key, .. } | ViewLine::CommentBody { id, key, .. },
            ) if !id.is_empty() => Some((id.clone(), !key.is_empty())),
            _ => None,
        }
    }

    /// Author of the comment with this forge-side id, for confirmations.
    fn comment_author(&self, id: &str) -> Option<String> {
        let session = self.pr.as_ref()?;
        session
            .data
            .threads
            .iter()
            .flat_map(|thread| &thread.comments)
            .chain(&session.data.conversation)
            .find(|comment| comment.id == id)
            .map(|comment| comment.author.clone())
    }

    /// Is the cursor on any comment-related row (head, body, or hint)?
    /// On these the comment micro-map consumes its keys even when they
    /// can't act, so `d`/`r` never fall back to jump/refresh mid-thread.
    fn on_comment_row(&self) -> bool {
        matches!(
            self.line_at_cursor(),
            Some(FlatLine::Line(
                ViewLine::CommentHead { .. }
                    | ViewLine::CommentBody { .. }
                    | ViewLine::CommentHint { .. }
            ))
        )
    }

    /// The comment-row micro-map: `d` deletes the comment under the
    /// cursor (pressed twice), `r` toggles the thread's resolution.
    /// Returns whether the key was consumed; away from comment rows both
    /// keys keep their global meaning (jump down / refresh).
    fn handle_comment_key(&mut self, code: KeyCode, pending_delete: Option<String>) -> bool {
        match code {
            KeyCode::Char('d') => {
                let Some((id, inline)) = self.comment_at_cursor() else {
                    // A hint row or an undeletable row (the description,
                    // section labels): a delete can't pick a comment from
                    // here — say so rather than jumping 15 lines.
                    if self.on_comment_row() {
                        self.notice =
                            Some("press d on the comment's author or text line".to_string());
                        return true;
                    }
                    return false;
                };
                if pending_delete.as_deref() == Some(id.as_str()) {
                    let spot = ActionSpot::CommentHead { id: id.clone() };
                    self.run_forge_mutation(
                        "deleting comment…",
                        "comment deleted",
                        Some(spot),
                        move |forge, detail| forge.delete_comment(detail.number, &id, inline),
                    );
                } else {
                    // Name whose comment is armed — from the hint row the
                    // target (the comment above) is otherwise implicit.
                    let whose = self
                        .comment_author(&id)
                        .map_or("this".to_string(), |author| format!("{author}'s"));
                    self.pending_delete = Some(id);
                    self.notice = Some(format!("press d again to delete {whose} comment"));
                }
                true
            }
            KeyCode::Char('r') => {
                let Some(key) = self.thread_key_at_cursor() else {
                    // Conversation comments aren't resolvable threads.
                    if self.on_comment_row() {
                        self.notice = Some("only review threads can be resolved".to_string());
                        return true;
                    }
                    return false;
                };
                let resolved = self
                    .pr
                    .as_ref()
                    .and_then(|s| s.data.threads.iter().find(|t| t.key == key))
                    .and_then(|t| t.resolved);
                let want = !matches!(resolved, Some(true));
                let spot = ActionSpot::ThreadHint { key: key.clone() };
                self.run_forge_mutation(
                    if want {
                        "resolving thread…"
                    } else {
                        "unresolving thread…"
                    },
                    if want {
                        "thread resolved"
                    } else {
                        "thread unresolved"
                    },
                    Some(spot),
                    move |forge, detail| forge.resolve(detail.number, &key, want),
                );
                true
            }
            _ => false,
        }
    }

    // --- composing & posting comments ---

    /// `a` / `A`: work out what the comment targets and queue the compose
    /// editor. `general` forces a PR-level comment from anywhere.
    fn request_comment(&mut self, general: bool) {
        if self.pr.is_none() {
            self.notice = Some("comments need an open pull request (p)".to_string());
            return;
        }
        let target = if general {
            Some(ComposeTarget::General)
        } else {
            self.compose_target_at_cursor()
        };
        let Some(target) = target else {
            // On a thread but not on its reply row: point at it instead
            // of failing silently.
            self.notice = Some(match self.thread_key_at_cursor() {
                Some(_) => "to reply, press a on the thread's [a] reply line".to_string(),
                None => "nothing to comment on here".to_string(),
            });
            return;
        };
        let title = self.compose_hint(&target);
        self.compose = Some(Compose::new(target, title));
    }

    /// All keys go to the composer while it's open: printable characters
    /// edit, enter posts, shift+enter (alt+enter in terminals that can't
    /// tell them apart) breaks the line, Esc discards.
    fn handle_compose_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.quit = true;
            }
            return;
        }
        let newline_modifier = key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        if key.code == KeyCode::Enter && !newline_modifier {
            let (target, body) = self.compose.take().expect("composing").into_body();
            if body.is_empty() {
                self.notice = Some("comment discarded (empty)".to_string());
            } else {
                self.post_comment(target, body);
            }
            return;
        }
        let Some(compose) = &mut self.compose else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.compose = None;
                self.notice = Some("comment discarded".to_string());
            }
            KeyCode::Enter => compose.insert('\n'),
            KeyCode::Backspace => compose.backspace(),
            KeyCode::Delete => compose.delete(),
            KeyCode::Left => compose.left(),
            KeyCode::Right => compose.right(),
            KeyCode::Up => compose.vertical(-1),
            KeyCode::Down => compose.vertical(1),
            KeyCode::Home => compose.home(),
            KeyCode::End => compose.end(),
            // Tabs render zero-width in the terminal; use spaces.
            KeyCode::Tab => "    ".chars().for_each(|ch| compose.insert(ch)),
            KeyCode::Char(ch) => compose.insert(ch),
            _ => {}
        }
    }

    /// The comment target under the cursor: the "[a] reply" hint row of a
    /// thread means a reply (only there — `a` elsewhere on a thread must
    /// not fire one by surprise), a diff line a new inline comment, the
    /// conversation view a general comment.
    fn compose_target_at_cursor(&self) -> Option<ComposeTarget> {
        if let Some(FlatLine::Line(ViewLine::CommentHint { key, .. })) = self.line_at_cursor()
            && !key.is_empty()
        {
            return Some(ComposeTarget::Reply {
                thread_key: key.clone(),
            });
        }
        if self.thread_key_at_cursor().is_some() {
            return None; // on a thread, but not on its reply row
        }
        let file = self.current_file()?;
        if pr::is_conversation(file) {
            return Some(ComposeTarget::General);
        }
        match self.line_at_cursor()? {
            FlatLine::Line(ViewLine::Diff { line, .. }) => Some(ComposeTarget::Inline {
                anchor: Anchor {
                    path: file.path.clone(),
                    old_path: file.old_path.clone(),
                    old_line: line.old_lineno,
                    new_line: line.new_lineno,
                    side: if line.new_lineno.is_some() {
                        Side::New
                    } else {
                        Side::Old
                    },
                },
            }),
            _ => None,
        }
    }

    /// One line describing the compose target — the composer's title.
    fn compose_hint(&self, target: &ComposeTarget) -> String {
        let session = self.pr.as_ref();
        match target {
            ComposeTarget::General => {
                let noun = self.noun();
                let number = session.map_or(0, |s| s.data.detail.number);
                format!("comment on {noun} #{number}")
            }
            ComposeTarget::Reply { thread_key } => {
                let author = session
                    .and_then(|s| s.data.threads.iter().find(|t| t.key == *thread_key))
                    .and_then(|t| t.comments.first())
                    .map_or("the thread", |c| c.author.as_str());
                format!("reply to {author}")
            }
            ComposeTarget::Inline { anchor } => {
                let line = anchor.new_line.or(anchor.old_line).unwrap_or(0);
                format!("comment on {}:{line}", anchor.path.display())
            }
        }
    }

    /// Run one comment mutation on a background thread; on success the
    /// thread refetches the comment side so the view can update in place
    /// via [`Self::on_pr_posted`]. `spot` is the code-view row the
    /// mutation acts on — the spinner renders there while waiting.
    fn run_forge_mutation<F>(&mut self, working: &str, done: &str, spot: Option<ActionSpot>, op: F)
    where
        F: FnOnce(&dyn Forge, &PrDetail) -> Result<(), ForgeError> + Send + 'static,
    {
        let Some(session) = &self.pr else {
            return;
        };
        let data = Arc::clone(&session.data);
        let Some((forge, seq, tx)) = self.begin_forge_request(spot, done) else {
            return;
        };
        self.notice = Some(working.to_string());
        std::thread::spawn(move || {
            let result = (|| -> Result<_, ForgeError> {
                let detail = &data.detail;
                op(forge.as_ref(), detail)?;
                Ok(Box::new(forge.threads(detail.number, detail)?))
            })()
            .map_err(|err| err.to_string());
            let _ = tx.send(AppEvent::PrPosted { seq, result });
        });
    }

    fn post_comment(&mut self, target: ComposeTarget, body: String) {
        let spot = match &target {
            ComposeTarget::General => ActionSpot::ConversationHint,
            ComposeTarget::Reply { thread_key } => ActionSpot::ThreadHint {
                key: thread_key.clone(),
            },
            ComposeTarget::Inline { anchor } => ActionSpot::DiffLine {
                path: anchor.path.clone(),
                old: anchor.old_line,
                new: anchor.new_line,
            },
        };
        self.run_forge_mutation(
            "posting comment…",
            "comment posted",
            Some(spot),
            move |forge, detail| match &target {
                ComposeTarget::General => forge.post_general(detail.number, &body),
                ComposeTarget::Reply { thread_key } => {
                    forge.reply(detail.number, thread_key, &body)
                }
                ComposeTarget::Inline { anchor } => forge.comment_inline(detail, anchor, &body),
            },
        );
    }

    /// A comment landed (or failed): swap in the refetched threads and
    /// recompute views without losing the reviewer's place.
    fn on_pr_posted(&mut self, seq: u64, result: Result<RefreshedComments, String>) -> Result<()> {
        if self.forge_request.as_ref().is_none_or(|r| r.seq != seq) {
            return Ok(()); // superseded by a newer forge request
        }
        let done_notice = self
            .forge_request
            .take()
            .map(|request| request.done_notice)
            .unwrap_or_default();
        let refreshed = match result {
            Ok(refreshed) => refreshed,
            Err(err) => {
                self.notice = Some(format!("failed: {err}"));
                return Ok(());
            }
        };
        let Some(session) = &mut self.pr else {
            return Ok(()); // session ended while posting
        };
        let (threads, conversation) = *refreshed;
        let mut data = (*session.data).clone();
        data.threads = threads;
        data.conversation = conversation;
        session.data = Arc::new(data);
        self.notice = Some(done_notice);
        // Threads may touch any file: recompute, keeping the cursor's line.
        let (current_path, lineno) = self.current_anchor();
        self.cache.reset();
        self.restore_place(current_path, lineno)?;
        self.start_prefetch();
        Ok(())
    }

    /// Switch the comparison base; on failure (e.g. no common ancestor)
    /// the old comparison stays and the error lands in the status bar.
    /// Returns whether `base` is the active base afterwards.
    fn set_base(&mut self, base: &str) -> Result<bool> {
        if base == self.cmp.base_label {
            return Ok(true);
        }
        match self.vcs.comparison(Some(base)) {
            Ok(cmp) => {
                self.cmp = cmp;
                self.reload()?;
                Ok(true)
            }
            Err(err) => {
                self.notice = Some(format!("cannot compare against '{base}': {err}"));
                Ok(false)
            }
        }
    }

    /// Narrow (or restore) which slice of the comparison is reviewed.
    fn set_scope(&mut self, scope: Scope) -> Result<()> {
        if scope == self.cmp.scope {
            return Ok(());
        }
        self.cmp.scope = scope;
        self.reload()
    }

    /// The mouse wheel scrolls whatever it hovers without moving cursors;
    /// clicks select/fold in the tree; dragging resizes on the divider or
    /// selects text in the code view.
    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        if self.help_open || self.picker.is_some() || self.compose.is_some() {
            return Ok(());
        }
        let position = Position::new(mouse.column, mouse.row);
        let in_tree = self.layout.tree_area.contains(position);
        let in_code = self.layout.code_area.contains(position);
        let tree_viewport = self.tree_viewport();
        let code_viewport = self.code_viewport();
        match mouse.kind {
            // The comparison segment at the status bar's left edge opens
            // the picker, mirroring the pick_base key (or, in a PR
            // session, the PR picker).
            MouseEventKind::Down(MouseButton::Left) if self.on_comparison_label(position) => {
                if self.pr.is_some() {
                    self.open_pr_picker();
                } else {
                    self.open_base_picker()?;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.layout.on_divider(position) => {
                self.layout.resizing = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.layout.resizing => {
                self.layout.drag(mouse.column);
            }
            MouseEventKind::Up(MouseButton::Left) if self.layout.resizing => {
                self.layout.resizing = false;
            }
            MouseEventKind::ScrollDown if in_tree => self.nav.scroll(3, tree_viewport),
            MouseEventKind::ScrollUp if in_tree => self.nav.scroll(-3, tree_viewport),
            MouseEventKind::ScrollDown if in_code => {
                self.code.scroll_view(3, code_viewport, self.view_len());
            }
            MouseEventKind::ScrollUp if in_code => {
                self.code.scroll_view(-3, code_viewport, self.view_len());
            }
            MouseEventKind::Down(MouseButton::Left) if in_tree => {
                self.focus = Pane::Tree;
                let row = (mouse.row - self.layout.tree_area.y) as usize + self.nav.offset();
                if row < self.nav.tree.visible_len() {
                    self.nav.set_cursor(row, tree_viewport);
                    if self.nav.selected_file().is_some() {
                        self.sync_current()?;
                    } else {
                        self.nav.toggle_dir();
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) if in_code => {
                self.focus = Pane::Code;
                let at = self.position_to_text(position);
                self.code.mouse_sel = Some((at, at));
            }
            MouseEventKind::Drag(MouseButton::Left) if self.code.mouse_sel.is_some() => {
                let at = self.position_to_text(position);
                if let Some((_, end)) = &mut self.code.mouse_sel {
                    *end = at;
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.code.mouse_sel.is_some() => {
                self.finish_mouse_selection();
            }
            _ => {}
        }
        Ok(())
    }

    // --- file navigation & review flow ---

    /// `j`/`k` (and `d`/`u`): move the cursor of whichever pane has
    /// focus. In the tree the shown diff follows the cursor, as always.
    fn move_focused(&mut self, delta: isize) -> Result<()> {
        match self.focus {
            Pane::Tree => {
                let viewport = self.tree_viewport();
                self.nav.move_cursor(delta, viewport, |_| false);
                self.sync_current()?;
            }
            Pane::Code => {
                let len = self.view_len();
                self.code.move_cursor(delta, len);
            }
        }
        Ok(())
    }

    /// `g`/`G` (with an optional count): jump within the focused pane.
    fn jump_focused(&mut self, target: usize) -> Result<()> {
        match self.focus {
            Pane::Tree => {
                let viewport = self.tree_viewport();
                let last = self.nav.tree.visible_len().saturating_sub(1);
                self.nav.set_cursor(target.min(last), viewport);
                self.sync_current()?;
            }
            Pane::Code => {
                let len = self.view_len();
                self.code.jump(target, len);
            }
        }
        Ok(())
    }

    fn move_file(&mut self, delta: isize) -> Result<()> {
        let viewport = self.tree_viewport();
        let files = &self.files;
        let review = &self.review;
        let skip = |file_index: usize| {
            files
                .get(file_index)
                .is_some_and(|f| review.contains(&f.path))
        };
        self.nav.move_cursor(delta, viewport, skip);
        self.sync_current()
    }

    /// Visible tree rows whose label contains the tree's query.
    fn match_rows(&self) -> Vec<usize> {
        if self.search_tree.is_empty() {
            return Vec::new();
        }
        let query = self.search_tree.to_lowercase();
        self.nav
            .tree
            .rows()
            .enumerate()
            .filter(|(_, node)| node.label.to_lowercase().contains(&query))
            .map(|(row, _)| row)
            .collect()
    }

    /// Rows where a change run (contiguous added/removed lines) starts —
    /// what `n`/`N` hop between when no code search is active.
    fn change_rows(&self) -> Vec<usize> {
        let Some(view) = self.current_view() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut in_change = false;
        for (row, flat) in view.flat_lines().enumerate() {
            let changed = matches!(
                flat,
                FlatLine::Line(ViewLine::Diff { line, .. }) if line.kind != LineKind::Context
            );
            if changed && !in_change {
                rows.push(row);
            }
            in_change = changed;
        }
        rows
    }

    /// Rows of the shown view whose content contains the code query.
    fn code_match_rows(&self) -> Vec<usize> {
        if self.search_code.is_empty() {
            return Vec::new();
        }
        let Some(view) = self.current_view() else {
            return Vec::new();
        };
        let query = self.search_code.to_lowercase();
        view.flat_lines()
            .enumerate()
            .filter(|(_, flat)| {
                flat.content()
                    .is_some_and(|content| content.to_lowercase().contains(&query))
            })
            .map(|(row, _)| row)
            .collect()
    }

    fn jump_to_first_match(&mut self) -> Result<()> {
        match self.focus {
            Pane::Tree => {
                if let Some(&row) = self.match_rows().first() {
                    self.nav.set_cursor(row, self.tree_viewport());
                    self.sync_current()?;
                }
            }
            Pane::Code => {
                if let Some(&row) = self.code_match_rows().first() {
                    let len = self.view_len();
                    self.code.jump(row, len);
                }
            }
        }
        Ok(())
    }

    /// `n`/`N`: cycle through the focused pane's search matches (each
    /// pane keeps its own query), wrapping around. In the code pane
    /// without a search they hop between the changes themselves.
    fn jump_match(&mut self, direction: isize) -> Result<()> {
        let rows = match self.focus {
            Pane::Tree => self.match_rows(),
            Pane::Code if self.search_code.is_empty() => self.change_rows(),
            Pane::Code => self.code_match_rows(),
        };
        if rows.is_empty() {
            self.notice = Some(match self.focus {
                Pane::Tree if self.search_tree.is_empty() => {
                    "no search here — press / first".to_string()
                }
                Pane::Code if self.search_code.is_empty() => {
                    "no changes to jump between".to_string()
                }
                _ => format!("no matches for '{}'", self.search_query()),
            });
            return Ok(());
        }
        let cursor = match self.focus {
            Pane::Tree => self.nav.cursor,
            Pane::Code => self.code.cursor,
        };
        let target = if direction > 0 {
            rows.iter().find(|&&row| row > cursor).or(rows.first())
        } else {
            rows.iter().rev().find(|&&row| row < cursor).or(rows.last())
        };
        if let Some(&row) = target {
            match self.focus {
                Pane::Tree => {
                    self.nav.set_cursor(row, self.tree_viewport());
                    self.sync_current()?;
                }
                Pane::Code => {
                    let len = self.view_len();
                    self.code.jump(row, len);
                }
            }
        }
        Ok(())
    }

    /// Check off the file under the cursor and advance to the next
    /// unreviewed one; pressing again unchecks (and stays).
    fn toggle_check(&mut self) -> Result<()> {
        let Some(index) = self.nav.selected_file() else {
            return Ok(());
        };
        if self.review.toggle(&self.files[index].path) {
            self.move_file(1)?;
        }
        Ok(())
    }

    /// `X`: pop the newest check and put the cursor back on that file.
    fn uncheck_last(&mut self) -> Result<()> {
        let Some(path) = self.review.pop_last() else {
            return Ok(());
        };
        let row = self
            .files
            .iter()
            .position(|f| f.path == path)
            .and_then(|index| self.nav.row_of_file(index));
        if let Some(row) = row {
            self.nav.set_cursor(row, self.tree_viewport());
            self.sync_current()?;
        }
        Ok(())
    }

    // --- view management ---

    /// Full reload: comparison stays, files and views recompute. Review
    /// checks survive (they're keyed by path).
    fn reload(&mut self) -> Result<()> {
        // A manual reload supersedes any in-flight live refresh.
        self.scan.cancel();
        self.files = self.vcs.changed_files(&self.cmp)?;
        self.nav.rebuild(&self.files);
        self.current = None;
        self.code.reset_for_new_view();
        self.cache.reset();
        self.sync_current()?;
        self.start_prefetch();
        Ok(())
    }

    /// Manual refresh (`r`): recompute all files and views, but keep the
    /// reviewer's place — unlike [`Self::reload`], which starts over
    /// (new comparison, new PR, leaving a session).
    fn refresh(&mut self) -> Result<()> {
        self.scan.cancel();
        let files = self.vcs.changed_files(&self.cmp)?;
        let (current_path, lineno) = self.current_anchor();
        self.files = files;
        self.nav
            .rebuild_preserving(&self.files, self.tree_viewport());
        self.cache.reset();
        self.restore_place(current_path, lineno)?;
        self.start_prefetch();
        Ok(())
    }

    /// Re-anchor after the file list was swapped out underneath the
    /// reviewer: the shown file is re-found by path, its cursor by line
    /// number; when the file is gone, fall back to the tree cursor.
    fn restore_place(&mut self, path: Option<PathBuf>, lineno: Option<u32>) -> Result<()> {
        self.current = path
            .as_ref()
            .and_then(|p| self.files.iter().position(|f| f.path == *p));
        if let Some(index) = self.current {
            self.ensure_view(index)?;
            self.reanchor_cursor(lineno);
        } else {
            self.code.reset_for_new_view();
            self.sync_current()?;
        }
        Ok(())
    }

    /// Put the code cursor back on `lineno` (or the nearest row after it)
    /// in the freshly computed view. Selections spanned content that may
    /// be gone; they never survive a re-anchor.
    fn reanchor_cursor(&mut self, lineno: Option<u32>) {
        if let Some(view) = self.current_view() {
            let last = view.flat_len().saturating_sub(1);
            self.code.cursor = lineno
                .and_then(|lineno| view.row_of_lineno(lineno))
                .unwrap_or(self.code.cursor)
                .min(last);
            self.code.select_anchor = None;
            self.code.mouse_sel = None;
        }
    }

    // --- live reload ---

    /// A debounced watcher batch arrived: remember what went stale and
    /// kick off (or queue) a background status scan.
    fn on_fs_changed(&mut self, paths: Vec<PathBuf>, meta: bool) {
        self.scan.dirty_paths.extend(paths);
        self.scan.meta_pending |= meta;
        // In a PR session the working tree isn't shown; what changed is
        // picked up by the full reload on leaving.
        if self.pr.is_some() {
            return;
        }
        if self.scan.inflight {
            self.scan.rescan_needed = true;
        } else {
            self.start_status_scan();
        }
    }

    /// Scan the working tree off the main thread; the result comes back
    /// as [`AppEvent::StatusReady`]. When git metadata moved the
    /// comparison is re-resolved too (commits, branch switches, rebases).
    fn start_status_scan(&mut self) {
        self.scan.inflight = true;
        let seq = self.scan.next_seq();
        let refresh_cmp = std::mem::take(&mut self.scan.meta_pending);
        let root = self.vcs.root().to_path_buf();
        let cmp = self.cmp.clone();
        let tx = self.events_tx.clone();
        std::thread::spawn(move || {
            let result = scan_status(&root, cmp, refresh_cmp);
            let _ = tx.send(AppEvent::StatusReady { seq, result });
        });
    }

    fn on_status_ready(
        &mut self,
        seq: u64,
        result: Result<(Comparison, Vec<ChangedFile>), String>,
    ) -> Result<()> {
        if seq != self.scan.seq {
            return Ok(()); // superseded by a reload or base switch
        }
        if self.pr.is_some() {
            // A PR session started while the scan ran and owns the file
            // list now; the reload on leaving covers what changed.
            self.scan.inflight = false;
            self.scan.rescan_needed = false;
            return Ok(());
        }
        self.scan.inflight = false;
        match result {
            Ok((cmp, files)) => {
                self.cmp = cmp;
                self.apply_refresh(files)?;
            }
            Err(err) => self.notice = Some(format!("refresh failed: {err}")),
        }
        if std::mem::take(&mut self.scan.rescan_needed) {
            self.start_status_scan();
        }
        Ok(())
    }

    /// Apply a background scan without losing the user's place: the tree
    /// keeps its cursor and collapsed dirs (matched by path), the shown
    /// file stays shown, and its cursor re-anchors by line number.
    fn apply_refresh(&mut self, files: Vec<ChangedFile>) -> Result<()> {
        // Anchor before anything moves.
        let (current_path, anchor) = self.current_anchor();
        self.files = files;
        self.nav
            .rebuild_preserving(&self.files, self.tree_viewport());
        let dirty: HashSet<PathBuf> = self.scan.dirty_paths.drain().collect();
        for path in &dirty {
            self.cache.remove(path);
        }
        let live: HashSet<&Path> = self.files.iter().map(|f| f.path.as_path()).collect();
        self.cache.retain(|path| live.contains(path));
        self.current = current_path
            .as_ref()
            .and_then(|p| self.files.iter().position(|f| f.path == *p));
        match (self.current, &current_path) {
            // The shown file changed on disk: recompute it now (one file,
            // milliseconds) and put the cursor back on the same line.
            (Some(index), Some(path)) if dirty.contains(path) => {
                self.ensure_view(index)?;
                self.reanchor_cursor(anchor);
            }
            (Some(_), _) => {} // untouched: the cached view is still valid
            (None, _) => {
                // The shown file left the changeset; fall back to the
                // file under the tree cursor.
                self.code.reset_for_new_view();
                self.sync_current()?;
            }
        }
        self.start_prefetch();
        Ok(())
    }

    /// Precompute views on background threads so navigation always hits a
    /// warm cache. Only files without a cached view are computed — under
    /// live reload most views survive a refresh, so this stays cheap.
    /// Work is interleaved across a small pool (worker k takes the k-th,
    /// k+N-th, … missing file) so the files near the top — where the
    /// cursor starts — warm first. Each worker opens its own repository
    /// handle; results stream in through the event channel and are
    /// discarded if the generation moved on.
    fn start_prefetch(&mut self) {
        self.generation += 1;
        let missing: Vec<usize> = (0..self.files.len())
            .filter(|&i| self.cache.get(&self.files[i].path).is_none())
            .collect();
        if missing.is_empty() {
            return;
        }
        let generation = self.generation;
        let workers = std::thread::available_parallelism()
            .map_or(1, |n| n.get() / 2)
            .clamp(1, 8)
            .min(missing.len());
        let root = self.vcs.root().to_path_buf();
        let files = Arc::new(self.files.clone());
        let missing = Arc::new(missing);
        let options = self.cache.options();
        // In a PR session the workers compute from the shared PR data
        // instead of the working tree.
        let session = self
            .pr
            .as_ref()
            .map(|s| (Arc::clone(&s.data), Arc::new(s.collapsed.clone())));
        for worker in 0..workers {
            let root = root.clone();
            let cmp = self.cmp.clone();
            let files = Arc::clone(&files);
            let missing = Arc::clone(&missing);
            let session = session.clone();
            let tx = self.events_tx.clone();
            std::thread::spawn(move || {
                let Ok(vcs) = vcs::detect(&root) else {
                    return;
                };
                let mut nth = worker;
                while nth < missing.len() {
                    let index = missing[nth];
                    let view = match &session {
                        Some((data, collapsed)) => Ok(pr::compute(
                            data,
                            collapsed,
                            &files[index],
                            vcs.as_ref(),
                            options,
                        )),
                        None => view_cache::compute(&files[index], vcs.as_ref(), &cmp, options),
                    };
                    if let Ok(view) = view
                        && tx
                            .send(AppEvent::ViewReady {
                                generation,
                                index,
                                view,
                            })
                            .is_err()
                    {
                        return; // app is gone
                    }
                    nth += workers;
                }
            });
        }
    }

    /// Load the view when the cursor lands on a different file; directory
    /// rows leave the current view untouched.
    fn sync_current(&mut self) -> Result<()> {
        let Some(index) = self.nav.selected_file() else {
            return Ok(());
        };
        if self.current == Some(index) {
            return Ok(());
        }
        self.current = Some(index);
        self.code.reset_for_new_view();
        self.ensure_view(index)
    }

    fn ensure_view(&mut self, index: usize) -> Result<()> {
        let Some(file) = self.files.get(index) else {
            return Ok(());
        };
        if let Some(session) = &self.pr {
            if self.cache.get(&file.path).is_none() {
                let view = pr::compute(
                    &session.data,
                    &session.collapsed,
                    file,
                    self.vcs.as_ref(),
                    self.cache.options(),
                );
                self.cache.insert_if_absent(file.path.clone(), view);
            }
            return Ok(());
        }
        self.cache.ensure(file, self.vcs.as_ref(), &self.cmp)?;
        Ok(())
    }

    /// Options changed: recompute the visible view, keep the cursor, and
    /// re-warm the rest in the background.
    fn reload_current_view(&mut self) -> Result<()> {
        self.cache.clear_views();
        self.code.select_anchor = None;
        if let Some(index) = self.current {
            self.ensure_view(index)?;
        }
        self.start_prefetch();
        Ok(())
    }

    /// Widen/narrow the global block scope, clamped to what the current
    /// file's view reports as available.
    fn adjust_scope(&mut self, delta: isize) -> Result<()> {
        if self.current.is_none() {
            return Ok(());
        }
        let scope_max = match self.current_view() {
            Some(FileView::Sections { scope_max, .. }) => *scope_max,
            _ => 0,
        };
        let current = self.cache.scope;
        let next = current.saturating_add_signed(delta).min(scope_max);
        if next != current {
            self.cache.scope = next;
            self.code.reset_for_new_view();
            // Global option changed: every cached view is stale.
            self.reload_current_view()?;
        }
        Ok(())
    }

    // --- copying ---

    /// Copy the shown file's absolute path to the clipboard.
    fn copy_path(&mut self) {
        let Some(path) = self
            .current_file()
            .map(|f| self.vcs.root().join(&f.path).display().to_string())
        else {
            return;
        };
        self.notice = Some(match copy_to_clipboard(&path) {
            Ok(()) => format!("copied {path}"),
            Err(err) => format!("copy failed: {err}"),
        });
    }

    /// Copy the current line — or the visual selection, which this ends —
    /// to the system clipboard.
    fn yank(&mut self) {
        let (from, to) = match self.code.select_anchor.take() {
            Some(anchor) => (anchor.min(self.code.cursor), anchor.max(self.code.cursor)),
            None => (self.code.cursor, self.code.cursor),
        };
        let text = self.view_text(from, to);
        if text.is_empty() {
            return;
        }
        let lines = to - from + 1;
        self.notice = Some(match copy_to_clipboard(&text) {
            Ok(()) => format!("yanked {lines} line{}", if lines == 1 { "" } else { "s" }),
            Err(err) => format!("copy failed: {err}"),
        });
    }

    /// Content of the flattened view lines `from..=to` (fold markers are
    /// skipped, section separators are blank lines).
    fn view_text(&self, from: usize, to: usize) -> String {
        let Some(view) = self.current_view() else {
            return String::new();
        };
        view.flat_lines()
            .enumerate()
            .filter(|(index, _)| (from..=to).contains(index))
            .filter_map(|(_, flat)| flat.content())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Screen position → (view line, char column); the gutter columns map
    /// to char 0.
    fn position_to_text(&self, position: Position) -> TextPos {
        let area = self.layout.code_area;
        let line = (position.y.saturating_sub(area.y)) as usize + self.code.scroll;
        let ch = position.x.saturating_sub(area.x + CODE_GUTTER) as usize;
        (line.min(self.view_len().saturating_sub(1)), ch)
    }

    fn finish_mouse_selection(&mut self) {
        let Some((start, end)) = self.code.mouse_selection() else {
            return;
        };
        self.code.mouse_sel = None;
        if start == end {
            return; // a plain click, not a drag
        }
        let text = self.selected_text(start, end);
        if text.is_empty() {
            return;
        }
        self.notice = Some(match copy_to_clipboard(&text) {
            Ok(()) => format!("copied {} chars", text.chars().count()),
            Err(err) => format!("copy failed: {err}"),
        });
    }

    /// Text between two (line, char) positions, inclusive of the end char.
    fn selected_text(&self, (l0, c0): TextPos, (l1, c1): TextPos) -> String {
        let Some(view) = self.current_view() else {
            return String::new();
        };
        let mut out: Vec<String> = Vec::new();
        for (index, flat) in view.flat_lines().enumerate() {
            if index < l0 || index > l1 {
                continue;
            }
            let Some(content) = flat.content() else {
                continue;
            };
            let start = if index == l0 {
                char_to_byte(content, c0)
            } else {
                0
            };
            let end = if index == l1 {
                char_to_byte(content, c1 + 1)
            } else {
                content.len()
            };
            if start <= end {
                out.push(content[start..end].to_string());
            }
        }
        out.join("\n")
    }
}

/// One background status scan, run off the main thread with its own
/// repository handle (gix handles aren't shared across threads). The
/// recovery policy is deliberately forgiving: mid-operation states
/// (rebase, unborn HEAD) can fail to re-resolve the comparison — keep
/// reviewing against the old ancestor; a scoped commit can vanish
/// (rebase, amend) — widen back to everything rather than failing.
fn scan_status(
    root: &Path,
    cmp: Comparison,
    refresh_cmp: bool,
) -> Result<(Comparison, Vec<ChangedFile>), String> {
    let vcs = vcs::detect(root).map_err(|e| e.to_string())?;
    let mut cmp = if refresh_cmp {
        match vcs.comparison(Some(&cmp.base_label)) {
            Ok(mut fresh) => {
                fresh.scope = cmp.scope.clone();
                fresh
            }
            Err(_) => cmp,
        }
    } else {
        cmp
    };
    let files = match vcs.changed_files(&cmp) {
        Ok(files) => files,
        Err(_) if cmp.scope != Scope::All => {
            cmp.scope = Scope::All;
            vcs.changed_files(&cmp).map_err(|e| e.to_string())?
        }
        Err(err) => return Err(err.to_string()),
    };
    Ok((cmp, files))
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text)?;
    Ok(())
}

/// Build the editor invocation from the config template: whitespace-split,
/// `{file}`/`{line}` substituted per argument, and the file path appended
/// when the template never mentions `{file}`.
fn editor_command(template: &str, path: &Path, line: u32) -> Option<std::process::Command> {
    let mut parts = template.split_whitespace();
    let mut command = std::process::Command::new(parts.next()?);
    let mut has_file = false;
    for part in parts {
        has_file |= part.contains("{file}");
        command.arg(
            part.replace("{file}", &path.display().to_string())
                .replace("{line}", &line.to_string()),
        );
    }
    if !has_file {
        command.arg(path);
    }
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(command: &std::process::Command) -> (String, Vec<String>) {
        (
            command.get_program().to_string_lossy().into_owned(),
            command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        )
    }

    #[test]
    fn editor_command_substitutes_and_appends() {
        let path = Path::new("/repo/src/main.rs");
        // Default shape: no {file} → the path is appended.
        let cmd = editor_command("nvim +{line}", path, 42).unwrap();
        assert_eq!(
            parts(&cmd),
            (
                "nvim".to_string(),
                vec!["+42".to_string(), "/repo/src/main.rs".to_string()]
            )
        );
        // Explicit {file}: substituted in place, nothing appended.
        let cmd = editor_command("code -g {file}:{line}", path, 7).unwrap();
        assert_eq!(
            parts(&cmd),
            (
                "code".to_string(),
                vec!["-g".to_string(), "/repo/src/main.rs:7".to_string()]
            )
        );
        // A bare program name still gets the file.
        let cmd = editor_command("vi", path, 1).unwrap();
        assert_eq!(
            parts(&cmd),
            ("vi".to_string(), vec!["/repo/src/main.rs".to_string()])
        );
        // An empty template is rejected.
        assert!(editor_command("  ", path, 1).is_none());
    }
}
