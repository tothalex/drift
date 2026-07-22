//! The app coordinator: session state (VCS, files, current file), the
//! event loop, and dispatch into the focused sub-states.
//!
//! Navigation is deliberately modeless: the tree is always the navigator,
//! the single code view always scrolls. No window focus, no prefixes.

pub mod code_view;
pub mod panes;
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
use crate::events::{AppEvent, INPUT_POLL_MS, spawn_input_thread, spawn_watcher_thread};
use crate::keymap::{Action, Keymap};
use crate::processor::view::{FileView, char_to_byte};
use crate::theme::Theme;
use crate::ui::CODE_GUTTER;
use crate::vcs::Vcs;
use crate::vcs::model::{ChangedFile, Comparison, Scope};

use code_view::{CodeView, TextPos};
use panes::PaneLayout;
use review::Review;
use tree_nav::TreeNav;
use view_cache::ViewCache;

/// The base-branch picker overlay: branch list and its cursor.
pub struct BasePicker {
    pub branches: Vec<String>,
    pub cursor: usize,
}

/// The scope picker overlay that follows a branch choice: review
/// everything, only untracked files, or one commit.
pub struct ScopePicker {
    pub entries: Vec<(Scope, String)>,
    pub cursor: usize,
}

/// Whichever picker overlay is open.
pub enum Picker {
    Base(BasePicker),
    Scope(ScopePicker),
}

impl Picker {
    fn move_cursor(&mut self, delta: isize) {
        let (cursor, len) = match self {
            Picker::Base(picker) => (&mut picker.cursor, picker.branches.len()),
            Picker::Scope(picker) => (&mut picker.cursor, picker.entries.len()),
        };
        *cursor = cursor
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }
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
    /// The `?` keybinding overlay is open; any key closes it.
    help_open: bool,
    /// The open picker overlay (base branch or review scope), if any.
    picker: Option<Picker>,
    /// Vim-style count prefix: typed digits repeat the next motion.
    count: Option<usize>,
    /// Tree search: the query (highlights persist until Esc)…
    search_query: String,
    /// …and whether `/` input mode is capturing keystrokes.
    search_input: bool,
    /// One-shot status message ("yanked 3 lines"); cleared on next key.
    notice: Option<String>,
    /// Event channel shared by the input thread and prefetch workers.
    events_tx: Sender<AppEvent>,
    events_rx: Receiver<AppEvent>,
    /// Bumped whenever cached views become stale; prefetch results from
    /// older generations are discarded on arrival.
    generation: u64,
    /// Live reload: paths the watcher flagged since the last applied
    /// refresh (their views are stale)…
    dirty_paths: HashSet<PathBuf>,
    /// …whether git metadata moved (the comparison itself may be stale)…
    meta_pending: bool,
    /// …and the background status-scan bookkeeping: results carry the
    /// sequence they were started with, stale ones are dropped.
    scan_seq: u64,
    scan_inflight: bool,
    /// Events arrived while a scan was running: go again when it lands.
    rescan_needed: bool,
    /// Editor command template ({file}/{line} placeholders).
    editor: String,
    /// While set, the input thread stops reading the terminal — an
    /// external editor owns it.
    input_paused: Arc<AtomicBool>,
    /// An editor launch requested by the last key, performed by the run
    /// loop (it needs the terminal handle).
    pending_editor: Option<(PathBuf, u32)>,
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
            help_open: false,
            picker: None,
            count: None,
            search_query: String::new(),
            search_input: false,
            notice: None,
            events_tx,
            events_rx,
            generation: 0,
            dirty_paths: HashSet::new(),
            meta_pending: false,
            scan_seq: 0,
            scan_inflight: false,
            rescan_needed: false,
            editor: config.editor,
            input_paused: Arc::new(AtomicBool::new(false)),
            pending_editor: None,
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

    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    /// The status bar's comparison segment, e.g. " main ← feature ".
    /// Also the click target that opens the base picker.
    pub fn comparison_label(&self) -> String {
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

    pub fn search_query(&self) -> &str {
        &self.search_query
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

    /// Called by the renderer once geometry is known.
    pub fn apply_pending_center(&mut self) {
        let viewport = self.layout.code_area.height as usize;
        let len = self.view_len();
        self.code.apply_pending_center(viewport, len);
    }

    // --- event loop ---

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
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
                AppEvent::FsChanged { paths, meta } => self.on_fs_changed(paths, meta),
                AppEvent::StatusReady { seq, result } => self.on_status_ready(seq, result),
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
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        let status = command.status();
        *terminal = ratatui::init();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
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
        // `/` input mode captures keystrokes; the cursor follows matches
        // live, yazi-style. Enter keeps the matches, Esc cancels.
        if self.search_input {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.quit = true;
                }
                KeyCode::Esc => {
                    self.search_input = false;
                    self.search_query.clear();
                }
                KeyCode::Enter => self.search_input = false,
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.jump_to_first_match()?;
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
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
            self.search_query.clear();
            return Ok(());
        }
        let Some(action) = self.keymap.action_for(key.code, key.modifiers) else {
            return Ok(());
        };
        let view_len = self.view_len();
        match action {
            // In visual mode `q` leaves the mode, like Esc — it must not
            // quit the app mid-selection.
            Action::Quit if self.code.select_anchor.is_some() => {
                self.code.select_anchor = None;
            }
            Action::Quit => self.quit = true,
            Action::Help => self.help_open = true,
            Action::NextFile => self.move_file(count)?,
            Action::PrevFile => self.move_file(-count)?,
            Action::ToggleDir => self.nav.toggle_dir(),
            Action::CursorDown => self.code.move_cursor(count, view_len),
            Action::CursorUp => self.code.move_cursor(-count, view_len),
            Action::JumpDown => self.code.move_cursor(15 * count, view_len),
            Action::JumpUp => self.code.move_cursor(-15 * count, view_len),
            Action::JumpTop => {
                let target = if explicit_count {
                    count as usize - 1
                } else {
                    0
                };
                self.code.jump(target, view_len);
            }
            Action::JumpBottom => {
                let target = if explicit_count {
                    count as usize - 1
                } else {
                    usize::MAX
                };
                self.code.jump(target, view_len);
            }
            Action::ScopeWiden => self.adjust_scope(count)?,
            Action::ScopeNarrow => self.adjust_scope(-count)?,
            Action::Search => {
                self.search_query.clear();
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
            Action::CheckFile => self.toggle_check()?,
            Action::UncheckLast => self.uncheck_last()?,
            Action::Visual => self.code.toggle_visual(),
            Action::Yank => self.yank(),
            Action::CopyPath => self.copy_path(),
            Action::GrowTree => self.layout.resize(2 * count),
            Action::ShrinkTree => self.layout.resize(-2 * count),
            Action::PickBase => self.open_base_picker()?,
            Action::Refresh => self.reload()?,
            Action::OpenEditor => self.request_editor(),
        }
        Ok(())
    }

    /// Is the position on the status bar's comparison segment? The status
    /// bar is the single row below the main area.
    fn on_comparison_label(&self, position: Position) -> bool {
        let status_row = self.layout.main_area.y + self.layout.main_area.height;
        position.y == status_row
            && (position.x as usize) < self.comparison_label().chars().count()
    }

    /// Queue the shown file for the editor, at the cursor's line.
    fn request_editor(&mut self) {
        let Some(file) = self.current_file() else {
            self.notice = Some("no file to open".to_string());
            return;
        };
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
                        self.open_scope_picker()?;
                    }
                }
                Some(Picker::Scope(picker)) => {
                    let scope = picker.entries[picker.cursor].0.clone();
                    self.set_scope(scope)?;
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

    fn open_scope_picker(&mut self) -> Result<()> {
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
        if self.help_open || self.picker.is_some() {
            return Ok(());
        }
        let position = Position::new(mouse.column, mouse.row);
        let in_tree = self.layout.tree_area.contains(position);
        let in_code = self.layout.code_area.contains(position);
        let tree_viewport = self.layout.tree_area.height as usize;
        let code_viewport = self.layout.code_area.height as usize;
        match mouse.kind {
            // The comparison segment at the status bar's left edge opens
            // the picker, mirroring the pick_base key.
            MouseEventKind::Down(MouseButton::Left) if self.on_comparison_label(position) => {
                self.open_base_picker()?;
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

    fn move_file(&mut self, delta: isize) -> Result<()> {
        let viewport = self.layout.tree_area.height as usize;
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

    /// Visible tree rows whose label contains the search query.
    fn match_rows(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            return Vec::new();
        }
        let query = self.search_query.to_lowercase();
        self.nav
            .tree
            .rows()
            .enumerate()
            .filter(|(_, node)| node.label.to_lowercase().contains(&query))
            .map(|(row, _)| row)
            .collect()
    }

    fn jump_to_first_match(&mut self) -> Result<()> {
        if let Some(&row) = self.match_rows().first() {
            self.nav
                .set_cursor(row, self.layout.tree_area.height as usize);
            self.sync_current()?;
        }
        Ok(())
    }

    /// `n`/`N`: cycle through search matches, wrapping around.
    fn jump_match(&mut self, direction: isize) -> Result<()> {
        let rows = self.match_rows();
        if rows.is_empty() {
            self.notice = Some(match self.search_query.is_empty() {
                true => "no search — press / first".to_string(),
                false => format!("no matches for '{}'", self.search_query),
            });
            return Ok(());
        }
        let cursor = self.nav.cursor;
        let target = if direction > 0 {
            rows.iter().find(|&&row| row > cursor).or(rows.first())
        } else {
            rows.iter().rev().find(|&&row| row < cursor).or(rows.last())
        };
        if let Some(&row) = target {
            self.nav
                .set_cursor(row, self.layout.tree_area.height as usize);
            self.sync_current()?;
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
            self.nav
                .set_cursor(row, self.layout.tree_area.height as usize);
            self.sync_current()?;
        }
        Ok(())
    }

    // --- view management ---

    /// Full reload: comparison stays, files and views recompute. Review
    /// checks survive (they're keyed by path).
    fn reload(&mut self) -> Result<()> {
        // A manual reload supersedes any in-flight live refresh.
        self.scan_seq += 1;
        self.scan_inflight = false;
        self.rescan_needed = false;
        self.meta_pending = false;
        self.dirty_paths.clear();
        self.files = self.vcs.changed_files(&self.cmp)?;
        self.nav.rebuild(&self.files);
        self.current = None;
        self.code.reset_for_new_view();
        self.cache.reset();
        self.sync_current()?;
        self.start_prefetch();
        Ok(())
    }

    // --- live reload ---

    /// A debounced watcher batch arrived: remember what went stale and
    /// kick off (or queue) a background status scan.
    fn on_fs_changed(&mut self, paths: Vec<PathBuf>, meta: bool) -> Result<()> {
        self.dirty_paths.extend(paths);
        self.meta_pending |= meta;
        if self.scan_inflight {
            self.rescan_needed = true;
        } else {
            self.start_status_scan();
        }
        Ok(())
    }

    /// Scan the working tree off the main thread; the result comes back
    /// as [`AppEvent::StatusReady`]. When git metadata moved the
    /// comparison is re-resolved too (commits, branch switches, rebases).
    fn start_status_scan(&mut self) {
        self.scan_inflight = true;
        self.scan_seq += 1;
        let seq = self.scan_seq;
        let refresh_cmp = std::mem::take(&mut self.meta_pending);
        let root = self.vcs.root().to_path_buf();
        let cmp = self.cmp.clone();
        let tx = self.events_tx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let vcs = crate::vcs::detect(&root).map_err(|e| e.to_string())?;
                let mut cmp = if refresh_cmp {
                    // Mid-operation states (rebase, unborn HEAD) can fail
                    // to resolve; keep reviewing against the old ancestor.
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
                    // A scoped commit can vanish (rebase, amend): widen
                    // back to everything rather than failing the refresh.
                    Err(_) if cmp.scope != Scope::All => {
                        cmp.scope = Scope::All;
                        vcs.changed_files(&cmp).map_err(|e| e.to_string())?
                    }
                    Err(err) => return Err(err.to_string()),
                };
                Ok((cmp, files))
            })();
            let _ = tx.send(AppEvent::StatusReady { seq, result });
        });
    }

    fn on_status_ready(
        &mut self,
        seq: u64,
        result: Result<(Comparison, Vec<ChangedFile>), String>,
    ) -> Result<()> {
        if seq != self.scan_seq {
            return Ok(()); // superseded by a reload or base switch
        }
        self.scan_inflight = false;
        match result {
            Ok((cmp, files)) => {
                self.cmp = cmp;
                self.apply_refresh(files)?;
            }
            Err(err) => self.notice = Some(format!("refresh failed: {err}")),
        }
        if std::mem::take(&mut self.rescan_needed) {
            self.start_status_scan();
        }
        Ok(())
    }

    /// Apply a background scan without losing the user's place: the tree
    /// keeps its cursor and collapsed dirs (matched by path), the shown
    /// file stays shown, and its cursor re-anchors by line number.
    fn apply_refresh(&mut self, files: Vec<ChangedFile>) -> Result<()> {
        let current_path = self
            .current
            .and_then(|i| self.files.get(i))
            .map(|f| f.path.clone());
        // Anchor before anything moves: the new-side line under the cursor.
        let anchor = self
            .current_view()
            .and_then(|view| view.lineno_at(self.code.cursor));
        self.files = files;
        self.nav
            .rebuild_preserving(&self.files, self.layout.tree_area.height as usize);
        let dirty: HashSet<PathBuf> = self.dirty_paths.drain().collect();
        for path in &dirty {
            self.cache.remove(path);
        }
        let live: HashSet<&std::path::Path> = self.files.iter().map(|f| f.path.as_path()).collect();
        self.cache.retain(|path| live.contains(path));
        self.current = current_path
            .as_ref()
            .and_then(|p| self.files.iter().position(|f| f.path == *p));
        match (self.current, &current_path) {
            // The shown file changed on disk: recompute it now (one file,
            // milliseconds) and put the cursor back on the same line.
            (Some(index), Some(path)) if dirty.contains(path) => {
                self.ensure_view(index)?;
                if let Some(view) = self.current_view() {
                    let last = view.flat_len().saturating_sub(1);
                    self.code.cursor = anchor
                        .and_then(|lineno| view.row_of_lineno(lineno))
                        .unwrap_or(self.code.cursor)
                        .min(last);
                    // Selections spanned content that no longer exists.
                    self.code.select_anchor = None;
                    self.code.mouse_sel = None;
                }
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
        for worker in 0..workers {
            let root = root.clone();
            let cmp = self.cmp.clone();
            let files = Arc::clone(&files);
            let missing = Arc::clone(&missing);
            let tx = self.events_tx.clone();
            std::thread::spawn(move || {
                let Ok(vcs) = crate::vcs::detect(&root) else {
                    return;
                };
                let mut nth = worker;
                while nth < missing.len() {
                    let index = missing[nth];
                    if let Ok(view) =
                        view_cache::compute(&files[index], vcs.as_ref(), &cmp, options)
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
        if let Some(file) = self.files.get(index) {
            self.cache.ensure(file, self.vcs.as_ref(), &self.cmp)?;
        }
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
