-- gclic in Neovim: gclic in a sidebar beside the code, which goes over the
-- whole editor while there's a diff, details or another tab to show, staying
-- alive while hidden, so it's in the same state each time it comes back.
-- Or, with `sidebar = false`, gclic over the whole editor only, toggled.
--
-- In the sidebar, the Changes pane shows a file as it's saved: the plugin
-- counts saves in a file, named in $GCLIC_SAVED, which gclic checks twice a
-- second. enter on a file shows its diff over the whole editor, and i on a
-- commit its details, and closing them goes back. z goes over the whole
-- editor too, staying there until z or ctrl-g goes back. gclic says which it
-- wants, as it knows what it shows,
-- and the plugin moves the one terminal buffer between the sidebar's split
-- and a float, so gclic keeps its state. ctrl-g in the sidebar hides gclic,
-- ctrl-h and ctrl-l go to the windows beside it, and the toggle shows it, or
-- goes into it.
--
-- gclic finds the Neovim it runs in through $NVIM. Its `o` calls `open` below,
-- which opens the file at the line beside the sidebar, or, without it, hides
-- gclic and opens the file in the window beneath. Its `:cd` changes Neovim's
-- working folder, so the file finder, search and file tree follow the repo
-- picked in gclic.
--
-- Without the sidebar, ctrl-g hides gclic without quitting it, and the toggle
-- brings it back as it was. q quits it, either way. Its diffs are in the
-- colour scheme's colours, as code is in the editor, while the rest of it
-- keeps the terminal's.
--
-- A bar across the top shows the repo in the colour gclic gives it, with its
-- name, as gclic's own does, following the working folder. The plugin has to
-- load at startup for it, as with lazy.nvim's `event = "VeryLazy"`. gclic
-- follows the working folder too: :cd in Neovim moves it to that repo.
--
-- To install it, see "Neovim" in the README, and `:checkhealth gclic`.

local M = {}

M.config = {
  -- The command that runs gclic: `gclic` on the PATH, or a path to it.
  cmd = "gclic",
  -- gclic in a sidebar at the left, this many columns wide, going over the
  -- whole editor for diffs, details and the other tabs. False for the whole
  -- editor only, sized below.
  sidebar = { width = 40 },
  -- The Repo tab's panes' width beside a diff over the whole editor, in
  -- columns: nil for the sidebar's, so they stay put as the diff opens, or
  -- gclic's fifth of the width without the sidebar.
  panes_width = nil,
  -- The sidebar and a file tree take turns at the left: opening one closes
  -- the other. Snacks's explorer, LazyVim's, neo-tree and nvim-tree.
  close_file_tree = true,
  -- Without the sidebar, the window's share of the editor, wide and high:
  -- all of it, but for the command line, unless set lower for a pop-up,
  -- which a border then suits.
  width = 1,
  height = 1,
  border = "none",
  -- A key that hides gclic from inside it too, where its keys are gclic's.
  -- One gclic doesn't use. False for none. With the sidebar, it goes back to
  -- the sidebar from the whole editor first, as gclic's own ctrl-g does.
  toggle_key = "<C-g>",
  -- Whose colours gclic's window takes: "terminal", the terminal Neovim runs
  -- in, so gclic looks as it does outside Neovim, whatever the editor's
  -- colour scheme, or "editor", the colour scheme's.
  colors = "terminal",
  -- Whose colours gclic's diffs take: "editor", the colour scheme's, so code
  -- looks as it does in the editor, or "gclic", VS Code Dark Modern's, which
  -- gclic has on its own.
  diff_colors = "editor",
  -- A bar across the top in the repo's colour, with its name, as gclic shows
  -- them, which gclic works out (`gclic --color`).
  accent = true,
}

-- The colours of the terminal Neovim runs in, as it answers when asked: its
-- 16 colours by number, and its foreground and background.
local terminal = { palette = {}, fg = nil, bg = nil }

-- The repo the working folder is in, and gclic's colour for it: nil for each
-- outside a repo, and for the colour while gclic's bar is turned off.
local accent = { color = nil, root = nil, asked = 0 }
-- The tabline as it was, to put back where there's no bar.
local plain_showtabline = nil

--- Whether there's a gclic to run.
local function installed()
  return vim.fn.executable(M.config.cmd) == 1
end

--- The bar, drawn as the tabline: a line in the repo's colour, then its
--- folder, dimmed, and its name, as gclic's tab bar has them.
function M.tabline()
  if not accent.root then
    return ""
  end
  local escape = function(text)
    return (text:gsub("%%", "%%%%"))
  end
  local name = vim.fn.fnamemodify(accent.root, ":t")
  local folder = vim.fn.fnamemodify(accent.root, ":~:h") .. "/"
  local label = " " .. folder .. name .. " "
  -- The middle third of a cell, as gclic's bar: level with the name, and
  -- thicker than a line character, whose thickness is up to the font.
  local line = string.rep("🬋", math.max(0, vim.o.columns - vim.fn.strdisplaywidth(label)))
  return "%#GclicAccentBar#" .. line .. "%#GclicAccentFolder# " .. escape(folder) .. "%#GclicAccentName#" .. escape(name) .. " "
end

--- Shows the bar for the repo as last found, or the tabline as it was.
local function draw_accent()
  if plain_showtabline == nil then
    plain_showtabline = vim.o.showtabline
  end
  if not (accent.color and accent.root) then
    vim.o.showtabline = plain_showtabline
    vim.o.tabline = ""
    return
  end
  local normal = vim.api.nvim_get_hl(0, { name = "Normal", link = false })
  -- The folder in the grey gclic dims it in, the terminal's colour 8, or else
  -- the line numbers', which colour schemes keep neutral.
  local line_number = vim.api.nvim_get_hl(0, { name = "LineNr", link = false })
  local folder = terminal.palette[8] or line_number.fg
  vim.api.nvim_set_hl(0, "GclicAccentBar", { fg = accent.color, bg = normal.bg })
  vim.api.nvim_set_hl(0, "GclicAccentName", { fg = accent.color, bg = normal.bg, bold = true })
  vim.api.nvim_set_hl(0, "GclicAccentFolder", { fg = folder, bg = normal.bg })
  vim.o.tabline = "%!v:lua.require'gclic'.tabline()"
  vim.o.showtabline = 2
end

--- Asks gclic for the working folder's repo and its colour, without waiting
--- for it, and shows them. gclic's `:color` calls this too, inside Neovim.
function M.refresh_accent()
  -- Without gclic, no bar, quietly: showing gclic says what's missing.
  if not M.config.accent or not vim.system or not installed() then
    return
  end
  -- Answers can come back out of order, after quick changes of folder: only
  -- the latest question's counts.
  accent.asked = accent.asked + 1
  local asked = accent.asked
  vim.system({ M.config.cmd, "--color" }, { text = true, cwd = vim.fn.getcwd() }, function(result)
    vim.schedule(function()
      if asked ~= accent.asked then
        return
      end
      local lines = vim.split(result.stdout or "", "\n")
      local found = result.code == 0
      accent.color = found and lines[1]:match("^#%x%x%x%x%x%x$") or nil
      accent.root = found and lines[2] ~= "" and lines[2] or nil
      draw_accent()
    end)
  end)
end

-- The colour scheme's colours for gclic's diffs: the editor's background and
-- text, its diff colours, and a colour for each kind of token, as the groups
-- gclic's theme.rs reads (its SCOPES), which this list mirrors: a test there
-- checks the two agree.
local DIFF_GROUPS = { { "Normal", "fg" }, { "Normal", "bg" }, { "DiffAdd", "bg" }, { "DiffDelete", "bg" } }
local SYNTAX_GROUPS = {
  "@comment", "@string", "@string.regexp", "@string.escape", "@constant", "@number",
  "@boolean", "@constant.builtin", "@keyword", "@keyword.function", "@keyword.import",
  "@keyword.return", "@operator", "@function", "@function.builtin", "@function.call",
  "@function.macro", "@type", "@type.builtin", "@variable", "@variable.parameter",
  "@variable.builtin", "@property", "@module", "@attribute", "@tag", "@tag.attribute",
  "@punctuation",
}

--- A group's colour for a part, "fg" or "bg", as `#rrggbb`. A group a colour
--- scheme leaves unset falls back to its parent, `@keyword.function` to
--- `@keyword`, as Neovim does, and a reversed one swaps its colours.
local function group_color(group, part)
  while group do
    local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = group, link = false })
    if ok then
      local key = part
      if hl.reverse then
        key = part == "fg" and "bg" or "fg"
      end
      if hl[key] then
        return string.format("#%06x", hl[key])
      end
    end
    group = group:match("^(.+)%.[^.]+$")
  end
end

--- The colours to give gclic, as its $GCLIC_EDITOR_COLORS: `group:part=#rrggbb`
--- pairs separated by `;`.
function M._editor_colors()
  local pairs_ = {}
  local function add(group, part)
    local color = group_color(group, part)
    if color then
      table.insert(pairs_, group .. ":" .. part .. "=" .. color)
    end
  end
  for _, entry in ipairs(DIFF_GROUPS) do
    add(entry[1], entry[2])
  end
  for _, group in ipairs(SYNTAX_GROUPS) do
    add(group, "fg")
  end
  return table.concat(pairs_, ";")
end

-- gclic's buffer; its windows: the sidebar, a split at the left, and the
-- float over the whole editor; which of the two it last asked for; and the
-- files it reads Neovim's working folder and saves from.
local state = {
  buf = nil,
  side = nil,
  win = nil,
  placeholder = nil,
  layout = "sidebar",
  follow = nil,
  saved = nil,
  saves = 0,
}

--- A colour from a terminal's answer, `rgb:` and 1 to 4 hex digits a
--- channel, as `#rrggbb`.
local function to_hex(r, g, b)
  local function channel(digits)
    return math.floor(tonumber(digits, 16) / (16 ^ #digits - 1) * 255 + 0.5)
  end
  return string.format("#%02x%02x%02x", channel(r), channel(g), channel(b))
end

--- Takes one answer: `ESC ] 4 ; n ; rgb:…` for colour n, and `ESC ] 10 ;`
--- or `11 ;` for the foreground or background.
function M._take_answer(sequence)
  local n, r, g, b = sequence:match("^\027%]4;(%d+);rgb:(%x+)/(%x+)/(%x+)")
  if n then
    terminal.palette[tonumber(n)] = to_hex(r, g, b)
    -- The bar's folder is in colour 8, which may come after it's drawn.
    if tonumber(n) == 8 and accent.root then
      vim.schedule(draw_accent)
    end
    return
  end
  local which
  which, r, g, b = sequence:match("^\027%](1[01]);rgb:(%x+)/(%x+)/(%x+)")
  if which == "10" then
    terminal.fg = to_hex(r, g, b)
  elseif which == "11" then
    terminal.bg = to_hex(r, g, b)
  end
end

--- Asks the terminal for its colours. The answers come back as TermResponse
--- events, which `_take_answer` reads.
local function ask_terminal()
  vim.api.nvim_create_autocmd("TermResponse", {
    group = vim.api.nvim_create_augroup("gclic_colors", { clear = true }),
    callback = function(ev)
      local sequence = type(ev.data) == "table" and ev.data.sequence or ev.data
      if type(sequence) == "string" then
        M._take_answer(sequence)
      end
    end,
  })
  local queries = { "\027]10;?\027\\", "\027]11;?\027\\" }
  for n = 0, 15 do
    table.insert(queries, string.format("\027]4;%d;?\027\\", n))
  end
  local send = vim.api.nvim_ui_send or function(text)
    io.stdout:write(text)
  end
  send(table.concat(queries))
end

--- The colours for gclic's windows: the terminal's, once they've come back,
--- or the editor's background otherwise, rather than a pop-up's.
local function color_window(win)
  local terminal_colors = M.config.colors == "terminal"
  if terminal_colors and not terminal.bg then
    -- The answers are quick, but may not be in yet on the first use.
    vim.wait(200, function()
      return terminal.bg ~= nil
    end)
  end
  if terminal_colors and terminal.bg then
    vim.api.nvim_set_hl(0, "GclicNormal", { fg = terminal.fg, bg = terminal.bg })
    vim.wo[win].winhighlight = "Normal:GclicNormal,NormalFloat:GclicNormal"
  else
    vim.wo[win].winhighlight = "NormalFloat:Normal"
  end
end

local function valid(win)
  return win ~= nil and vim.api.nvim_win_is_valid(win)
end

local function valid_win()
  return valid(state.win)
end

local function valid_buf()
  return state.buf ~= nil and vim.api.nvim_buf_is_valid(state.buf)
end

--- Whether gclic opens in the sidebar, rather than over the whole editor.
local function sidebar_on()
  return type(M.config.sidebar) == "table"
end

local function sidebar_width()
  return M.config.sidebar.width or 40
end

--- Whether gclic shows, in either window.
local function shown()
  return valid_win() or (valid(state.side) and vim.api.nvim_win_get_buf(state.side) == state.buf)
end

--- Where the window goes, without the sidebar, for the editor's size now. A
--- border takes a row and a column on each side, outside the window.
local function placement()
  local columns, lines = vim.o.columns, vim.o.lines - vim.o.cmdheight
  local edge = M.config.border == "none" and 0 or 2
  local width = math.max(1, math.floor(columns * M.config.width) - edge)
  local height = math.max(1, math.floor(lines * M.config.height) - edge)
  return {
    relative = "editor",
    width = width,
    height = height,
    col = math.floor((columns - width - edge) / 2),
    row = math.floor((lines - height - edge) / 2),
  }
end

--- With the sidebar, the window over the whole editor starts where the
--- sidebar does and is as high, so gclic's panes stay where they were, and
--- the tabline, status line and command line stay too.
local function full_placement()
  local row, col = unpack(vim.api.nvim_win_get_position(state.side))
  return {
    relative = "editor",
    row = row,
    col = col,
    width = math.max(1, vim.o.columns - col),
    height = vim.api.nvim_win_get_height(state.side),
  }
end

--- Whether gclic's window covers the whole editor, as it does unless set
--- smaller, for a pop-up. Not with the sidebar, which leaves Neovim's lines.
local function full_screen()
  return not sidebar_on() and M.config.width >= 1 and M.config.height >= 1
end

--- Full screen, Neovim's command line goes while gclic shows, as no window
--- can cover it, and comes back when gclic is hidden. Messages still show
--- when there are any.
local function hide_command_line()
  if full_screen() and state.cmdheight == nil then
    state.cmdheight = vim.o.cmdheight
    vim.o.cmdheight = 0
  end
end

local function restore_command_line()
  if state.cmdheight ~= nil then
    vim.o.cmdheight = state.cmdheight
    state.cmdheight = nil
  end
end

--- Keeps gclic's lines from scrolling sideways, by the mouse too while the
--- code has the focus: wrapped, they're pinned to the first column, though
--- they're as wide as the window anyway. A terminal starts without it.
local function pin(win)
  vim.wo[win].wrap = true
  vim.api.nvim_win_call(win, function()
    vim.fn.winrestview({ leftcol = 0 })
  end)
end

-- The file trees' windows, by filetype, besides Snacks's explorer, which
-- Snacks lists.
local TREES = { ["neo-tree"] = true, NvimTree = true }

--- Snacks's explorers open on this tab.
local function explorers()
  local ok, picker = pcall(require, "snacks.picker")
  if not ok or type(picker.get) ~= "function" then
    return {}
  end
  local got, list = pcall(picker.get, { source = "explorer" })
  return got and list or {}
end

--- Closes the file trees, for the sidebar to take their place.
local function close_file_trees()
  if not M.config.close_file_tree then
    return
  end
  for _, explorer in ipairs(explorers()) do
    pcall(function()
      explorer:close()
    end)
  end
  local function tree_windows()
    return vim.tbl_filter(function(win)
      local filetype = vim.bo[vim.api.nvim_win_get_buf(win)].filetype
      return TREES[filetype] or filetype == "snacks_layout_box"
    end, vim.api.nvim_tabpage_list_wins(0))
  end
  for _, win in ipairs(tree_windows()) do
    if TREES[vim.bo[vim.api.nvim_win_get_buf(win)].filetype] then
      pcall(vim.api.nvim_win_close, win, false)
    end
  end
  -- Snacks closes its windows a moment later: the sidebar waits, to open in
  -- their place at its own width, rather than beside them.
  vim.wait(500, function()
    return #tree_windows() == 0
  end, 10)
end

--- Whether `buf` is a file tree's: neo-tree's or nvim-tree's, by its
--- filetype, or one of Snacks's while an explorer is open, as all its
--- pickers' windows share filetypes, and the explorer's float in a split.
local function is_file_tree(buf)
  local filetype = vim.bo[buf].filetype
  if TREES[filetype] then
    return true
  end
  return filetype:match("^snacks_") ~= nil and #explorers() > 0
end

--- An empty buffer in gclic's colours, for the sidebar's window to hold
--- while gclic shows over the whole editor, so the code beside it doesn't
--- move.
local function placeholder()
  if not (state.placeholder and vim.api.nvim_buf_is_valid(state.placeholder)) then
    state.placeholder = vim.api.nvim_create_buf(false, true)
    vim.bo[state.placeholder].filetype = "gclic"
    -- Going into it goes to gclic, where it shows.
    vim.api.nvim_create_autocmd("BufEnter", {
      buffer = state.placeholder,
      callback = function()
        if valid_win() then
          vim.schedule(function()
            if valid_win() then
              vim.api.nvim_set_current_win(state.win)
            end
          end)
        end
      end,
    })
  end
  return state.placeholder
end

--- The sidebar's window, opened at the left if it isn't open, holding `buf`.
--- Neovim's terminal takes the size of the smallest window it shows in, so
--- gclic is never in both.
local function open_side(buf)
  if not valid(state.side) then
    close_file_trees()
    local current = vim.api.nvim_get_current_win()
    vim.cmd("topleft " .. sidebar_width() .. "vsplit")
    state.side = vim.api.nvim_get_current_win()
    vim.api.nvim_win_set_buf(state.side, buf)
    local wo = vim.wo[state.side]
    wo.winfixwidth = true
    wo.number = false
    wo.relativenumber = false
    wo.signcolumn = "no"
    wo.foldcolumn = "0"
    wo.statuscolumn = ""
    wo.cursorline = false
    wo.colorcolumn = ""
    wo.spell = false
    wo.list = false
    wo.wrap = true
    color_window(state.side)
    local side = state.side
    vim.api.nvim_create_autocmd("WinClosed", {
      pattern = tostring(side),
      once = true,
      callback = function()
        if state.side == side then
          state.side = nil
        end
      end,
    })
    if vim.api.nvim_win_is_valid(current) then
      vim.api.nvim_set_current_win(current)
    end
  elseif vim.api.nvim_win_get_buf(state.side) ~= buf then
    vim.api.nvim_win_set_buf(state.side, buf)
  end
  pin(state.side)
  return state.side
end

--- Opens the float, without the sidebar or over the whole editor with it.
local function open_float(options)
  options.style = "minimal"
  options.border = sidebar_on() and "none" or M.config.border
  state.win = vim.api.nvim_open_win(state.buf, true, options)
  pin(state.win)
  local win = state.win
  -- However the window closes, :quit included, the command line comes back.
  vim.api.nvim_create_autocmd("WinClosed", {
    pattern = tostring(win),
    once = true,
    callback = function()
      restore_command_line()
      if state.win == win then
        state.win = nil
      end
    end,
  })
  color_window(state.win)
end

local function close_float()
  if valid_win() then
    vim.api.nvim_win_hide(state.win)
  end
  state.win = nil
end

--- Shows gclic in the sidebar, and goes into it if `focus`.
local function show_sidebar(focus)
  local side = open_side(state.buf)
  close_float()
  state.layout = "sidebar"
  if focus then
    vim.api.nvim_set_current_win(side)
    vim.cmd.startinsert()
  end
end

--- Shows gclic over the whole editor, the sidebar holding its place.
local function show_full()
  open_side(placeholder())
  if valid_win() then
    vim.api.nvim_win_set_config(state.win, full_placement())
    vim.api.nvim_set_current_win(state.win)
  else
    open_float(full_placement())
  end
  state.layout = "full"
  vim.cmd.startinsert()
end

--- Shows gclic as it was last, and goes into it.
local function show()
  if not sidebar_on() then
    hide_command_line()
    open_float(placement())
    vim.cmd.startinsert()
  elseif state.layout == "full" then
    show_full()
  else
    show_sidebar(true)
  end
end

--- Hides gclic, which keeps running.
function M.hide()
  close_float()
  if valid(state.side) then
    -- Not the last window, which Neovim keeps: gclic leaves it.
    if #vim.tbl_filter(function(win)
      return vim.api.nvim_win_get_config(win).relative == ""
    end, vim.api.nvim_tabpage_list_wins(0)) > 1 then
      vim.api.nvim_win_close(state.side, true)
    else
      vim.api.nvim_win_set_buf(state.side, vim.api.nvim_create_buf(true, false))
    end
  end
  state.side = nil
  restore_command_line()
end

--- Called by gclic in the sidebar: shows it over the whole editor, `full`,
--- while it has a diff, details or another tab to show, or in the sidebar.
function M._layout(full)
  if not (valid_buf() and sidebar_on()) then
    return
  end
  local layout = full and "full" or "sidebar"
  -- Hidden, say after a refresh closed its diff, gclic shows there next.
  if layout == state.layout or not shown() then
    state.layout = layout
    return
  end
  if full then
    show_full()
  else
    show_sidebar(true)
  end
end

--- Counts a save, for gclic to show the change at once.
local function count_save()
  if state.saved then
    state.saves = state.saves + 1
    vim.fn.writefile({ tostring(state.saves) }, state.saved)
  end
end

--- Starts gclic in the working folder, or shows it again where it was.
function M.show()
  if valid_win() and not sidebar_on() then
    vim.api.nvim_set_current_win(state.win)
    vim.cmd.startinsert()
    return
  end
  if valid_buf() then
    if state.layout == "full" and valid_win() then
      vim.api.nvim_set_current_win(state.win)
      vim.cmd.startinsert()
    else
      show()
    end
    return
  end
  if not installed() then
    vim.notify(
      "gclic: can't find `" .. M.config.cmd .. "`. Install gclic, or set the plugin's `cmd`. See :checkhealth gclic",
      vim.log.levels.ERROR
    )
    return
  end
  state.buf = vim.api.nvim_create_buf(false, true)
  state.layout = "sidebar"
  show()
  -- Before gclic starts, as a terminal takes its colours when it does.
  if M.config.colors == "terminal" then
    for n, color in pairs(terminal.palette) do
      vim.b[state.buf]["terminal_color_" .. n] = color
    end
  end
  -- Neovim's working folder, for gclic to follow, as `:cd` there changes it,
  -- and a count of the files saved, for gclic to show them at once.
  state.follow = vim.fn.tempname()
  vim.fn.writefile({ vim.fn.getcwd() }, state.follow)
  state.saved = vim.fn.tempname()
  state.saves = 0
  vim.fn.writefile({ "0" }, state.saved)
  local follow, saved = state.follow, state.saved
  local on_exit = function()
    -- gclic quit: its windows, buffer and files go with it.
    vim.schedule(function()
      vim.fn.delete(follow)
      vim.fn.delete(saved)
      if state.follow == follow then
        state.follow = nil
        state.saved = nil
      end
      M.hide()
      for _, buf in ipairs({ state.buf, state.placeholder }) do
        if buf and vim.api.nvim_buf_is_valid(buf) then
          vim.api.nvim_buf_delete(buf, { force = true })
        end
      end
      state.buf = nil
      state.placeholder = nil
      state.layout = "sidebar"
    end)
  end
  -- The colour scheme's colours as gclic starts: a later change of colour
  -- scheme shows the next time it does.
  -- And this Neovim, for gclic to call back into, as `nvim` on the PATH may
  -- be another.
  local env = { GCLIC_FOLLOW = follow, GCLIC_SAVED = saved, GCLIC_NVIM = vim.v.progpath }
  if sidebar_on() then
    env.GCLIC_SIDEBAR_WIDTH = tostring(sidebar_width())
  end
  local panes_width = M.config.panes_width or (sidebar_on() and sidebar_width())
  if panes_width then
    env.GCLIC_PANES_WIDTH = tostring(panes_width)
  end
  if M.config.diff_colors == "editor" then
    env.GCLIC_EDITOR_COLORS = M._editor_colors()
  end
  -- In gclic's window, wherever it shows.
  vim.api.nvim_win_call(vim.fn.bufwinid(state.buf), function()
    if vim.fn.has("nvim-0.11") == 1 then
      state.job = vim.fn.jobstart({ M.config.cmd }, { term = true, env = env, on_exit = on_exit })
    else
      state.job = vim.fn.termopen({ M.config.cmd }, { env = env, on_exit = on_exit })
    end
  end)
  pin(vim.fn.bufwinid(state.buf))
  -- And wherever it shows next.
  vim.api.nvim_create_autocmd("BufWinEnter", {
    buffer = state.buf,
    callback = function()
      pin(vim.api.nvim_get_current_win())
    end,
  })
  -- A filetype of its own, for configs to tell gclic's window apart, say to
  -- keep a status line or winbar out of it.
  vim.bo[state.buf].filetype = "gclic"
  local keys = { buffer = state.buf }
  if M.config.toggle_key then
    if not sidebar_on() then
      vim.keymap.set("t", M.config.toggle_key, M.hide, { buffer = state.buf, desc = "Hide gclic" })
    elseif M.config.toggle_key ~= "<C-g>" then
      -- In the sidebar, gclic takes ctrl-g itself: back to the sidebar from
      -- the whole editor, or hidden. Another key stands in for it.
      vim.keymap.set("t", M.config.toggle_key, function()
        vim.api.nvim_chan_send(state.job, "\007")
      end, { buffer = state.buf, desc = "Back to the sidebar, or hide gclic" })
    end
  end
  -- From the sidebar, ctrl-h and ctrl-l go to the windows beside it, as they
  -- do between Neovim's windows. Over the whole editor there are none.
  if sidebar_on() then
    for key, direction in pairs({ ["<C-h>"] = "h", ["<C-l>"] = "l" }) do
      vim.keymap.set("t", key, function()
        if state.layout == "sidebar" then
          vim.cmd.wincmd(direction)
        end
      end, { buffer = state.buf, desc = "Go to the window beside gclic" })
    end
  end
  -- gclic doesn't use the mouse, so the mouse reaches Neovim, whose scrolling,
  -- up, down or sideways, would leave terminal mode and scroll the window,
  -- shifting gclic's screen and sending its keys to Neovim. A trackpad keeps
  -- scrolling for a moment after a flick, into gclic if it opens meanwhile.
  -- So in gclic's window, the mouse does nothing.
  local mouse = { "LeftMouse", "LeftDrag", "LeftRelease", "2-LeftMouse", "RightMouse", "MiddleMouse" }
  for _, wheel in ipairs({ "ScrollWheelUp", "ScrollWheelDown", "ScrollWheelLeft", "ScrollWheelRight" }) do
    for _, modifier in ipairs({ "", "S-", "C-", "M-" }) do
      table.insert(mouse, modifier .. wheel)
    end
  end
  for _, event in ipairs(mouse) do
    vim.keymap.set({ "t", "n" }, "<" .. event .. ">", "<Nop>", keys)
  end
  -- The hide key works outside terminal mode too, so gclic can't get stuck.
  if M.config.toggle_key then
    vim.keymap.set("n", M.config.toggle_key, M.hide, { buffer = state.buf, desc = "Hide gclic" })
  end
  -- And should gclic's window leave terminal mode anyway, coming back to it
  -- goes back into it, at the live screen.
  vim.api.nvim_create_autocmd({ "BufEnter", "WinEnter" }, {
    buffer = state.buf,
    callback = function()
      if vim.api.nvim_get_mode().mode ~= "t" then
        vim.cmd.startinsert()
      end
    end,
  })
end

--- The toggle. With the sidebar: shows gclic, or goes into it where it
--- shows; inside it, the key is gclic's, and ctrl-g there hides it.
function M.toggle()
  if not sidebar_on() then
    if valid_win() then
      M.hide()
    else
      M.show()
    end
    return
  end
  local current = vim.api.nvim_get_current_win()
  if shown() and (current == state.win or current == state.side) then
    return
  end
  M.show()
end

--- The window to open files in beside the sidebar: the one to its right,
--- or a new one if there's none.
local function code_window()
  local right = vim.api.nvim_win_call(state.side, function()
    return vim.fn.win_getid(vim.fn.winnr("l"))
  end)
  if right ~= 0 and right ~= state.side then
    return right
  end
  vim.api.nvim_set_current_win(state.side)
  vim.cmd("rightbelow vnew")
  local win = vim.api.nvim_get_current_win()
  vim.api.nvim_win_set_width(state.side, sidebar_width())
  return win
end

--- Called by gclic's `o`: opens the file at the line. Beside the sidebar,
--- which stays, and from the whole editor back in the sidebar. Without the
--- sidebar, gclic hides and the file opens in the window beneath it.
function M.open(file, line)
  if sidebar_on() and valid_buf() and shown() then
    show_sidebar(false)
    vim.api.nvim_set_current_win(code_window())
  else
    M.hide()
  end
  vim.cmd.edit(vim.fn.fnameescape(file))
  pcall(vim.api.nvim_win_set_cursor, 0, { line, 0 })
  vim.cmd("normal! zz")
  vim.cmd.stopinsert()
end

function M.setup(opts)
  M.config = vim.tbl_deep_extend("force", M.config, opts or {})
  vim.api.nvim_create_user_command("Gclic", M.toggle, { desc = "Toggle gclic" })
  if M.config.colors == "terminal" and #vim.api.nvim_list_uis() > 0 then
    ask_terminal()
  end
  -- A change of working folder, for gclic, if it's running, to follow. Not
  -- one window's own, as with :lcd, as the rest of Neovim stays put.
  vim.api.nvim_create_autocmd("DirChanged", {
    group = vim.api.nvim_create_augroup("gclic_follow", { clear = true }),
    callback = function()
      if state.follow and vim.v.event.scope ~= "window" then
        vim.fn.writefile({ vim.fn.getcwd() }, state.follow)
      end
    end,
  })
  -- A file tree opening hides the sidebar, whose place it takes.
  vim.api.nvim_create_autocmd({ "FileType", "BufWinEnter" }, {
    group = vim.api.nvim_create_augroup("gclic_trees", { clear = true }),
    callback = function(ev)
      if not (M.config.close_file_tree and sidebar_on() and valid(state.side)) then
        return
      end
      -- Once the tree's window is up and Snacks has listed its explorer.
      vim.schedule(function()
        if vim.api.nvim_buf_is_valid(ev.buf) and valid(state.side) and is_file_tree(ev.buf) then
          M.hide()
        end
      end)
    end,
  })
  -- A file saved, or changed by a shell command or outside while Neovim was
  -- away, for gclic to show at once rather than on its next refresh.
  vim.api.nvim_create_autocmd({ "BufWritePost", "FocusGained", "ShellCmdPost" }, {
    group = vim.api.nvim_create_augroup("gclic_saved", { clear = true }),
    callback = count_save,
  })
  if M.config.accent then
    local group = vim.api.nvim_create_augroup("gclic_accent", { clear = true })
    vim.api.nvim_create_autocmd("DirChanged", { group = group, callback = M.refresh_accent })
    -- A new colour scheme clears the bar's colours.
    vim.api.nvim_create_autocmd("ColorScheme", { group = group, callback = draw_accent })
    M.refresh_accent()
  end
  -- The windows follow the terminal's size, and gclic redraws to fit them.
  vim.api.nvim_create_autocmd("VimResized", {
    group = vim.api.nvim_create_augroup("gclic", { clear = true }),
    callback = function()
      if valid_win() then
        local options = sidebar_on() and valid(state.side) and full_placement() or placement()
        vim.api.nvim_win_set_config(state.win, options)
      end
      if valid(state.side) then
        vim.api.nvim_win_set_width(state.side, sidebar_width())
      end
    end,
  })
end

return M
