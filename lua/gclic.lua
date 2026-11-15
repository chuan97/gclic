-- gclic in Neovim: gclic in a floating window, toggled with a key, staying
-- alive while hidden, so it's in the same state each time it comes back.
--
-- gclic finds the Neovim it runs in through $NVIM. Its `o` calls `open` below,
-- which hides gclic and opens the file at the line in the window beneath, and
-- its `:cd` changes Neovim's working folder, so the file finder, search and
-- file tree follow the repo picked in gclic.
--
-- ctrl-g hides gclic without quitting it, and the toggle brings it back as it
-- was; q quits it. Its diffs are in the colour scheme's colours, as code is in
-- the editor, while the rest of it keeps the terminal's.
--
-- A bar across the top shows the repo in the colour gclic gives it, with its
-- name, as gclic's own does, following the working folder. The plugin has to
-- load at startup for it, as with lazy.nvim's `event = "VeryLazy"`. gclic
-- follows the working folder too: :cd in Neovim moves it to that repo.
--
-- With lazy.nvim:
--
--   {
--     "chuan97/gclic",
--     keys = { { "<leader>gg", function() require("gclic").toggle() end, desc = "gclic" } },
--     opts = {},
--   }

local M = {}

M.config = {
  -- The command that runs gclic.
  cmd = "gclic",
  -- The window's share of the editor, wide and high: all of it, but for the
  -- command line, unless set lower for a pop-up, which a border then suits.
  width = 1,
  height = 1,
  border = "none",
  -- A key that hides gclic from inside it too, where its keys are gclic's.
  -- One gclic doesn't use. False for none.
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
  local line = string.rep("━", math.max(0, vim.o.columns - vim.fn.strdisplaywidth(label)))
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
  if not M.config.accent or not vim.system then
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
-- gclic's theme.rs reads (its SCOPES), which this list mirrors.
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

-- gclic's buffer and window, and the file it reads Neovim's working folder
-- from, to follow it to another repo.
local state = { buf = nil, win = nil, follow = nil }


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

--- Dresses gclic's buffer and window in the terminal's colours, once they've
--- come back, or the editor's background otherwise, rather than a pop-up's.
local function color_window()
  local terminal_colors = M.config.colors == "terminal"
  if terminal_colors and not terminal.bg then
    -- The answers are quick, but may not be in yet on the first use.
    vim.wait(200, function()
      return terminal.bg ~= nil
    end)
  end
  if terminal_colors and terminal.bg then
    vim.api.nvim_set_hl(0, "GclicNormal", { fg = terminal.fg, bg = terminal.bg })
    vim.wo[state.win].winhighlight = "Normal:GclicNormal,NormalFloat:GclicNormal"
  else
    vim.wo[state.win].winhighlight = "NormalFloat:Normal"
  end
end

local function valid_win()
  return state.win ~= nil and vim.api.nvim_win_is_valid(state.win)
end

local function valid_buf()
  return state.buf ~= nil and vim.api.nvim_buf_is_valid(state.buf)
end

--- Where the window goes, for the editor's size now. A border takes a row
--- and a column on each side, outside the window.
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

--- Whether gclic's window covers the whole editor, as it does unless set
--- smaller, for a pop-up.
local function full_screen()
  return M.config.width >= 1 and M.config.height >= 1
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

local function show()
  hide_command_line()
  local options = placement()
  options.style = "minimal"
  options.border = M.config.border
  state.win = vim.api.nvim_open_win(state.buf, true, options)
  -- However the window closes, :quit included, the command line comes back.
  vim.api.nvim_create_autocmd("WinClosed", {
    pattern = tostring(state.win),
    once = true,
    callback = restore_command_line,
  })
  color_window()
  vim.cmd.startinsert()
end

--- Hides gclic, which keeps running.
function M.hide()
  if valid_win() then
    vim.api.nvim_win_hide(state.win)
  end
  state.win = nil
  restore_command_line()
end

--- Starts gclic in the working folder, or shows it again where it was.
function M.show()
  if valid_win() then
    vim.api.nvim_set_current_win(state.win)
    vim.cmd.startinsert()
    return
  end
  if valid_buf() then
    show()
    return
  end
  state.buf = vim.api.nvim_create_buf(false, true)
  show()
  -- Before gclic starts, as a terminal takes its colours when it does.
  if M.config.colors == "terminal" then
    for n, color in pairs(terminal.palette) do
      vim.b[state.buf]["terminal_color_" .. n] = color
    end
  end
  -- Neovim's working folder, for gclic to follow, as `:cd` there changes it.
  state.follow = vim.fn.tempname()
  vim.fn.writefile({ vim.fn.getcwd() }, state.follow)
  local follow = state.follow
  local on_exit = function()
    -- gclic quit: its window, buffer and file go with it.
    vim.schedule(function()
      vim.fn.delete(follow)
      if state.follow == follow then
        state.follow = nil
      end
      M.hide()
      if valid_buf() then
        vim.api.nvim_buf_delete(state.buf, { force = true })
      end
      state.buf = nil
    end)
  end
  -- The colour scheme's colours as gclic starts: a later change of colour
  -- scheme shows the next time it does.
  local env = { GCLIC_FOLLOW = state.follow }
  if M.config.diff_colors == "editor" then
    env.GCLIC_EDITOR_COLORS = M._editor_colors()
  end
  if vim.fn.has("nvim-0.11") == 1 then
    vim.fn.jobstart({ M.config.cmd }, { term = true, env = env, on_exit = on_exit })
  else
    vim.fn.termopen({ M.config.cmd }, { env = env, on_exit = on_exit })
  end
  if M.config.toggle_key then
    vim.keymap.set("t", M.config.toggle_key, M.hide, { buffer = state.buf, desc = "Hide gclic" })
  end
  -- gclic doesn't use the mouse, so the mouse reaches Neovim, whose scroll
  -- wheel would leave terminal mode and scroll back through the terminal's
  -- history, with gclic's keys then going to Neovim. The wheel moves gclic's
  -- cursor instead, as j and k do, and clicks do nothing.
  local keys = { buffer = state.buf }
  vim.keymap.set("t", "<ScrollWheelDown>", "j", keys)
  vim.keymap.set("t", "<ScrollWheelUp>", "k", keys)
  for _, click in ipairs({ "LeftMouse", "LeftDrag", "LeftRelease", "2-LeftMouse", "RightMouse", "MiddleMouse" }) do
    vim.keymap.set("t", "<" .. click .. ">", "<Nop>", keys)
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

function M.toggle()
  if valid_win() then
    M.hide()
  else
    M.show()
  end
end

--- Called by gclic's `o`: hides gclic and opens the file at the line, in the
--- window beneath, where the user was before opening gclic.
function M.open(file, line)
  M.hide()
  vim.cmd.edit(vim.fn.fnameescape(file))
  pcall(vim.api.nvim_win_set_cursor, 0, { line, 0 })
  vim.cmd("normal! zz")
end

function M.setup(opts)
  M.config = vim.tbl_extend("force", M.config, opts or {})
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
  if M.config.accent then
    local group = vim.api.nvim_create_augroup("gclic_accent", { clear = true })
    vim.api.nvim_create_autocmd("DirChanged", { group = group, callback = M.refresh_accent })
    -- A new colour scheme clears the bar's colours.
    vim.api.nvim_create_autocmd("ColorScheme", { group = group, callback = draw_accent })
    M.refresh_accent()
  end
  -- The window follows the terminal's size, and gclic redraws to fit it.
  vim.api.nvim_create_autocmd("VimResized", {
    group = vim.api.nvim_create_augroup("gclic", { clear = true }),
    callback = function()
      if valid_win() then
        vim.api.nvim_win_set_config(state.win, placement())
      end
    end,
  })
end

return M
