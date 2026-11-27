-- The Neovim plugin's sidebar, end to end, in headless Neovim with a real
-- gclic: opening, saving, going over the whole editor and back, resizing, `o`
-- from each, wrapping, taking turns with a file tree, hiding and quitting.
-- CI runs it on macOS. Run it from the repo's root, after a build:
--
--   nvim --headless --clean -l tests/nvim/sidebar.lua target/debug/gclic
--
-- It prints each check and exits 1 at the first that fails.

local root = vim.fn.getcwd()
local gclic = vim.fn.fnamemodify(arg[1] or "target/debug/gclic", ":p")
vim.opt.rtp:prepend(root)
vim.o.columns, vim.o.lines = 160, 40

-- A repo with a committed file, as the editor would have open.
local repo = vim.fn.tempname()
vim.fn.mkdir(repo, "p")
local function git(...)
  vim.system({ "git", "-C", repo, ... }):wait()
end
git("init", "-q")
git("config", "user.email", "test@example.com")
git("config", "user.name", "Test")
vim.fn.writefile({ "one", "two" }, repo .. "/notes.txt")
git("add", ".")
git("commit", "-qm", "First")
vim.cmd.cd(repo)

local gc = require("gclic")
gc.setup({ cmd = gclic, colors = "editor", accent = false })
vim.cmd.edit("notes.txt")
local code = vim.api.nvim_get_current_win()

local failed = false
local function check(what, ok)
  print((ok and "ok   " or "FAIL ") .. what)
  if not ok then
    failed = true
    vim.cmd("cquit 1")
  end
end

--- gclic's terminal buffer, and the windows: the sidebar's and the float.
local function term()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].buftype == "terminal" then
      return buf
    end
  end
end
local function float()
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_config(win).relative ~= "" then
      return win
    end
  end
end
local function side()
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_config(win).relative == "" and vim.bo[vim.api.nvim_win_get_buf(win)].filetype == "gclic" then
      return win
    end
  end
end
local function screen()
  return table.concat(vim.api.nvim_buf_get_lines(term(), 0, -1, false), "\n")
end
local function wait(what, condition)
  check(what, vim.wait(5000, condition, 20))
end
local function keys(text)
  vim.api.nvim_chan_send(vim.bo[term()].channel, text)
end

-- The selection's colour: a list's selected item's, unless it hardly stands
-- out from the background, as CursorLine often doesn't.
vim.api.nvim_set_hl(0, "Normal", { fg = "#d4d4d4", bg = "#1f1f1f" })
vim.api.nvim_set_hl(0, "CursorLine", { bg = "#222222" })
vim.api.nvim_set_hl(0, "Visual", { bg = "#264f78" })
vim.api.nvim_set_hl(0, "PmenuSel", { bg = "#04395e" })
check("the selection takes PmenuSel's colour", gc._selected_bg() == "#04395e")
vim.api.nvim_set_hl(0, "PmenuSel", { bg = "#202020" })
check("one too close to the background is passed over", gc._selected_bg() == "#264f78")
vim.api.nvim_set_hl(0, "PmenuSel", {})
vim.api.nvim_set_hl(0, "Visual", {})
check("with only a faint CursorLine, gclic keeps its grey", gc._selected_bg() == nil)
-- The interface's colours, by group, for gclic to pick what each is for.
vim.api.nvim_set_hl(0, "WinSeparator", { fg = "#444444" })
vim.api.nvim_set_hl(0, "DiagnosticError", { fg = "#f44747" })
local ui = gc._ui_colors()
check("the interface's colours name their groups", ui:find("WinSeparator:fg=#444444", 1, true) ~= nil
  and ui:find("DiagnosticError:fg=#f44747", 1, true) ~= nil and ui:find("Normal:fg=#d4d4d4", 1, true) ~= nil)

-- Opening: the sidebar at the left, 40 wide, in focus, the code beside it.
gc.toggle()
wait("gclic draws in the sidebar", function()
  return term() and screen():find("Changes") ~= nil
end)
check("the sidebar is at the left, 40 wide", vim.api.nvim_win_get_position(side())[2] == 0
  and vim.api.nvim_win_get_width(side()) == 40)
check("the sidebar holds gclic and has the focus", vim.api.nvim_win_get_buf(side()) == term()
  and vim.api.nvim_get_current_win() == side())
check("the code stays beside it", vim.api.nvim_win_is_valid(code))
check("nothing floats", float() == nil)

-- The toggle again, from inside: nothing changes.
gc.toggle()
check("the toggle inside gclic leaves it", vim.api.nvim_get_current_win() == side() and float() == nil)
-- From the code, it goes back into gclic.
vim.api.nvim_set_current_win(code)
gc.toggle()
check("the toggle from the code goes into gclic", vim.api.nvim_get_current_win() == side())

-- A save shows in Changes. gclic's own tests check it shows at once while
-- idle; here, that the plugin's save reaches it.
vim.api.nvim_win_call(code, function()
  vim.api.nvim_buf_set_lines(0, 0, -1, false, { "one", "two", "three" })
  vim.cmd.write()
end)
wait("the save shows in Changes", function()
  return screen():find("notes.txt") ~= nil
end)

-- enter on the file: gclic asks for the whole editor, from the sidebar's row.
vim.api.nvim_set_current_win(side())
keys("\r")
wait("enter goes over the whole editor", function()
  return float() ~= nil
end)
local f = float()
local row, col = unpack(vim.api.nvim_win_get_position(side()))
local config = vim.api.nvim_win_get_config(f)
check("the float starts where the sidebar does, as high", config.row == row and config.col == col
  and vim.api.nvim_win_get_height(f) == vim.api.nvim_win_get_height(side()))
check("the float is as wide as the editor", vim.api.nvim_win_get_width(f) == 160)
check("the sidebar's window holds its place, without gclic", vim.api.nvim_win_get_buf(side()) ~= term())
check("the float has the focus", vim.api.nvim_get_current_win() == f)
wait("the diff shows right of the panes", function()
  return screen():find("three") ~= nil
end)
-- The panes keep the sidebar's 40 columns beside it: the diff's pane starts
-- at the 41st.
local first_row = vim.api.nvim_buf_get_lines(term(), 1, 2, false)[1] or ""
check("the panes stay 40 wide beside the diff", vim.fn.strcharpart(first_row, 40, 1) == "╭")

-- Resizing the editor resizes the float.
vim.o.columns = 140
vim.api.nvim_exec_autocmds("VimResized", {})
check("the float follows the editor's width", vim.api.nvim_win_get_width(float()) == 140)

-- esc closes the diff, and gclic goes back to the sidebar.
keys("\027")
wait("esc goes back to the sidebar", function()
  return float() == nil and side() and vim.api.nvim_win_get_buf(side()) == term()
end)
check("the sidebar has the focus again", vim.api.nvim_get_current_win() == side())

-- o from the sidebar opens the file beside it, which stays.
keys("o")
wait("o from the sidebar opens the file beside it", function()
  return vim.api.nvim_get_current_win() == code
end)
check("the sidebar stays", side() ~= nil and vim.api.nvim_win_get_buf(side()) == term())
check("the code window has the file", vim.fn.bufname(vim.api.nvim_win_get_buf(code)):find("notes.txt") ~= nil)

-- o from the whole editor: back to the sidebar, and the file beside it.
vim.api.nvim_set_current_win(side())
keys("z")
wait("z goes over the whole editor", function()
  return float() ~= nil
end)
keys("o")
wait("o from the whole editor goes back to the sidebar", function()
  return float() == nil and vim.api.nvim_get_current_win() == code
end)
check("the sidebar holds gclic again", vim.api.nvim_win_get_buf(side()) == term())

-- After z, closing a diff stays over the whole editor, and z goes back.
vim.api.nvim_set_current_win(side())
keys("z")
wait("z goes over the whole editor, opening nothing", function()
  return float() ~= nil
end)
keys("\r")
vim.wait(300)
keys("\027")
vim.wait(300)
keys("\027")
vim.wait(600)
check("closing a diff after z stays over the whole editor", float() ~= nil)
keys("z")
wait("z with nothing open goes back to the sidebar", function()
  return float() == nil and vim.api.nvim_win_get_buf(side()) == term()
end)

-- ctrl-g: from the whole editor to the sidebar, from the sidebar hidden.
vim.api.nvim_set_current_win(side())
keys("z")
wait("z again", function()
  return float() ~= nil
end)
keys("\007")
wait("ctrl-g goes back to the sidebar", function()
  return float() == nil and side() and vim.api.nvim_win_get_buf(side()) == term()
end)
keys("\007")
wait("ctrl-g in the sidebar hides gclic", function()
  return side() == nil and float() == nil
end)
check("gclic keeps running while hidden", term() ~= nil)
gc.toggle()
check("the toggle brings the sidebar back", side() ~= nil and vim.api.nvim_win_get_buf(side()) == term())

-- gclic's lines never scroll sideways, by the mouse either while the code has
-- the focus: wrapped, in the sidebar too, after its trips over the editor.
check("the sidebar's lines are wrapped", vim.wo[side()].wrap)

-- A file tree and the sidebar take turns at the left.
vim.api.nvim_set_current_win(code)
vim.cmd("topleft vnew")
vim.bo.filetype = "neo-tree"
local tree = vim.api.nvim_get_current_win()
wait("a file tree opening hides the sidebar", function()
  return side() == nil
end)
check("gclic keeps running behind the tree", term() ~= nil)
gc.toggle()
check("the sidebar closes the file tree", not vim.api.nvim_win_is_valid(tree))
check("the sidebar comes back 40 wide", side() ~= nil and vim.api.nvim_win_get_width(side()) == 40)
-- Snacks's pickers share the explorer's filetypes, but without an explorer
-- open, a picker isn't a tree.
vim.api.nvim_set_current_win(code)
local picker = vim.api.nvim_create_buf(false, true)
vim.bo[picker].filetype = "snacks_picker_list"
local picker_win = vim.api.nvim_open_win(picker, true, { relative = "editor", row = 2, col = 60, width = 30, height = 5 })
vim.wait(200)
check("a picker leaves the sidebar", side() ~= nil)
vim.api.nvim_win_close(picker_win, true)
-- ctrl-e in gclic opens the file tree in its place.
check("ctrl-e in gclic is mapped", vim.api.nvim_buf_call(term(), function()
  return vim.fn.maparg("<C-e>", "t", false, true).buffer == 1
end))
check(":checkhealth finds no file tree, in a bare Neovim", gc._file_tree_status().tree == nil)
gc.config.file_tree = function()
  vim.cmd("topleft vnew")
  vim.bo.filetype = "neo-tree"
end
check(":checkhealth names the file_tree function", gc._file_tree_status().tree == "your file_tree function")
gc.file_tree()
local opened = vim.api.nvim_get_current_win()
check("the file tree opens, and gclic goes", side() == nil and vim.bo[vim.api.nvim_win_get_buf(opened)].filetype == "neo-tree")
gc.config.file_tree = nil
gc.toggle()
check("gclic comes back in the tree's place", side() ~= nil and not vim.api.nvim_win_is_valid(opened))

-- Quitting from the whole editor closes both windows and the buffer.
keys("z")
wait("z once more", function()
  return float() ~= nil
end)
keys("q")
vim.wait(200)
keys("\r")
wait("quitting closes gclic's windows and buffer", function()
  return term() == nil and float() == nil and side() == nil
end)
check("the code is left", vim.api.nvim_win_is_valid(code))

-- And quitting from the sidebar.
gc.toggle()
wait("gclic starts again", function()
  return term() and screen():find("Changes") ~= nil
end)
keys("q")
vim.wait(200)
keys("\r")
wait("quitting from the sidebar closes it", function()
  return term() == nil and side() == nil
end)

-- Without the sidebar, as before: over the whole editor, and o hides gclic.
gc.setup({ cmd = gclic, colors = "editor", accent = false, sidebar = false })
vim.api.nvim_set_current_win(code)
gc.toggle()
wait("without the sidebar, gclic opens over the whole editor", function()
  return float() and term() and screen():find("Changes") ~= nil
end)
check("without the sidebar, there's no split", side() == nil)
gc.toggle()
check("without the sidebar, the toggle hides gclic", float() == nil and term() ~= nil)
gc.toggle()
keys("o")
wait("without the sidebar, o hides gclic and opens the file", function()
  return float() == nil and vim.api.nvim_get_current_win() == code
end)
gc.toggle()
keys("q")
vim.wait(200)
keys("\r")
wait("without the sidebar, quitting closes it", function()
  return term() == nil and float() == nil
end)

vim.fn.delete(repo, "rf")
if not failed then
  print("all checks passed")
end
vim.cmd("qall!")
