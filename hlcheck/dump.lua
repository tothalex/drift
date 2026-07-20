-- Dump neovim's resolved foreground color for every character of a file,
-- as `row<TAB>col<TAB>char<TAB>#rrggbb<TAB>src` where src is `lsp` when an
-- LSP semantic token set the color, else `ts`. Run headless:
--   nvim --headless -u <init.lua> -l dump.lua <file> <out.tsv>
local target, outpath = arg[1], arg[2]
vim.cmd('edit ' .. vim.fn.fnameescape(target))
local buf = vim.api.nvim_get_current_buf()
-- Let :edit pick the filetype, then start tree-sitter for that language.
pcall(vim.treesitter.start, buf)
-- Give an attached language server a moment to publish semantic tokens.
vim.wait(700)

local function fg(group)
  local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = group, link = false })
  if ok and hl and hl.fg then
    return string.format('#%06x', hl.fg)
  end
  return '-'
end

local lines = {}
for row = 0, vim.api.nvim_buf_line_count(buf) - 1 do
  local text = vim.api.nvim_buf_get_lines(buf, row, row + 1, false)[1]
  for col = 0, #text - 1 do
    local pos = vim.inspect_pos(buf, row, col, {
      syntax = false,
      semantic_tokens = true,
      treesitter = true,
      extmarks = false,
    })
    local ts = pos.treesitter or {}
    local cap = ts[#ts] -- most specific (last) tree-sitter capture
    local sem = (pos.semantic_tokens or {})[1]
    local ts_fg = cap and fg(cap.hl_group) or '-'
    local sem_fg = sem and fg(sem.opts and sem.opts.hl_group or sem.hl_group) or '-'
    -- Semantic tokens paint over tree-sitter, so they are what shows.
    local shown = (sem_fg ~= '-') and sem_fg or ts_fg
    lines[#lines + 1] = string.format(
      '%d\t%d\t%s\t%s\t%s',
      row + 1, col, text:sub(col + 1, col + 1), shown, (sem_fg ~= '-') and 'lsp' or 'ts'
    )
  end
end
vim.fn.writefile(lines, outpath)
