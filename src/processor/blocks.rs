//! The block-resolution contract.

/// A semantic block (function, class, if, loop, …) in the new-side source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// 1-based inclusive line range in the new file.
    pub range: (u32, u32),
    /// Display title, e.g. the block's signature line.
    pub title: String,
}

/// Resolves the semantic blocks enclosing a changed line range, innermost
/// first (e.g. `[if, fn, impl]`). Empty when the change sits at top level.
///
/// Implemented by the tree-sitter resolver; an indentation-heuristic
/// resolver for unsupported languages can slot in later.
pub trait BlockResolver {
    fn enclosing_blocks(&self, changed: (u32, u32)) -> Vec<Block>;
}
