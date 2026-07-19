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

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::DefaultTerminal;
use ratatui::layout::Position;

use crate::config::Config;
use crate::events::{AppEvent, spawn_input_thread};
use crate::keymap::{Action, Keymap};
use crate::processor::view::{FileView, char_to_byte};
use crate::theme::Theme;
use crate::ui::CODE_GUTTER;
use crate::vcs::Vcs;
use crate::vcs::model::{ChangedFile, Comparison};

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
    /// The base-branch picker overlay, when open.
    picker: Option<BasePicker>,
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
        self.current.and_then(|i| self.cache.get(i))
    }

    pub fn help_open(&self) -> bool {
        self.help_open
    }

    pub fn picker(&self) -> Option<&BasePicker> {
        self.picker.as_ref()
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
        spawn_input_thread(self.events_tx.clone());
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
                    if generation == self.generation {
                        self.cache.insert_if_absent(index, view);
                    }
                    Ok(())
                }
            };
            // After startup, failures (e.g. git during a rebase) surface in
            // the status bar instead of exiting the app.
            if let Err(err) = result {
                self.notice = Some(format!("error: {err:#}"));
            }
        }
        Ok(())
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
        }
        Ok(())
    }

    /// Picker keys are a fixed modal micro-map: j/k/arrows move, Enter
    /// selects, Esc/q cancel.
    fn handle_picker_key(&mut self, code: KeyCode) -> Result<()> {
        let Some(picker) = &mut self.picker else {
            return Ok(());
        };
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.picker = None,
            KeyCode::Char('k') | KeyCode::Up => picker.cursor = picker.cursor.saturating_sub(1),
            KeyCode::Char('j') | KeyCode::Down => {
                picker.cursor = (picker.cursor + 1).min(picker.branches.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                let base = picker.branches[picker.cursor].clone();
                self.picker = None;
                self.set_base(&base)?;
            }
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
        self.picker = Some(BasePicker { branches, cursor });
        Ok(())
    }

    /// Switch the comparison base; on failure (e.g. no common ancestor)
    /// the old comparison stays and the error lands in the status bar.
    fn set_base(&mut self, base: &str) -> Result<()> {
        if base == self.cmp.base_label {
            return Ok(());
        }
        match self.vcs.comparison(Some(base)) {
            Ok(cmp) => {
                self.cmp = cmp;
                self.reload()?;
            }
            Err(err) => self.notice = Some(format!("cannot compare against '{base}': {err}")),
        }
        Ok(())
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
        self.files = self.vcs.changed_files(&self.cmp)?;
        self.nav.rebuild(&self.files);
        self.current = None;
        self.code.reset_for_new_view();
        self.cache.reset();
        self.sync_current()?;
        self.start_prefetch();
        Ok(())
    }

    /// Precompute every file's view on background threads so navigation
    /// always hits a warm cache. Work is interleaved across a small pool
    /// (worker k takes indices k, k+N, …) so the files near the top —
    /// where the cursor starts — warm first. Each worker opens its own
    /// repository handle; results stream in through the event channel and
    /// are discarded if the generation moved on.
    fn start_prefetch(&mut self) {
        self.generation += 1;
        if self.files.is_empty() {
            return;
        }
        let generation = self.generation;
        let workers = std::thread::available_parallelism()
            .map_or(1, |n| n.get() / 2)
            .clamp(1, 8)
            .min(self.files.len());
        let root = self.vcs.root().to_path_buf();
        let files = Arc::new(self.files.clone());
        let options: Arc<Vec<_>> = Arc::new(
            (0..files.len())
                .map(|index| self.cache.options_for(index))
                .collect(),
        );
        for worker in 0..workers {
            let root = root.clone();
            let cmp = self.cmp.clone();
            let files = Arc::clone(&files);
            let options = Arc::clone(&options);
            let tx = self.events_tx.clone();
            std::thread::spawn(move || {
                let Ok(vcs) = crate::vcs::detect(&root) else {
                    return;
                };
                let mut index = worker;
                while index < files.len() {
                    if let Ok(view) =
                        view_cache::compute(&files[index], vcs.as_ref(), &cmp, options[index])
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
                    index += workers;
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
            self.cache
                .ensure(index, file, self.vcs.as_ref(), &self.cmp)?;
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
        let Some(index) = self.current else {
            return Ok(());
        };
        let scope_max = match self.cache.get(index) {
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
