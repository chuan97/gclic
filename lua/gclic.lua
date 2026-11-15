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
}

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

local state = { buf = nil, win = nil }

-- The colours of the terminal Neovim runs in, as it answers when asked: its
-- 16 colours by number, and its foreground and background.
local terminal = { palette = {}, fg = nil, bg = nil }

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

local function show()
  local options = placement()
  options.style = "minimal"
  options.border = M.config.border
  state.win = vim.api.nvim_open_win(state.buf, true, options)
  color_window()
  vim.cmd.startinsert()
end

--- Hides gclic, which keeps running.
function M.hide()
  if valid_win() then
    vim.api.nvim_win_hide(state.win)
  end
  state.win = nil
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
  local on_exit = function()
    -- gclic quit: its window and buffer go with it.
    vim.schedule(function()
      M.hide()
      if valid_buf() then
        vim.api.nvim_buf_delete(state.buf, { force = true })
      end
      state.buf = nil
    end)
  end
  -- The colour scheme's colours as gclic starts: a later change of colour
  -- scheme shows the next time it does.
  local env = nil
  if M.config.diff_colors == "editor" then
    env = { GCLIC_EDITOR_COLORS = M._editor_colors() }
  end
  if vim.fn.has("nvim-0.11") == 1 then
    vim.fn.jobstart({ M.config.cmd }, { term = true, env = env, on_exit = on_exit })
  else
    vim.fn.termopen({ M.config.cmd }, { env = env, on_exit = on_exit })
  end
  if M.config.toggle_key then
    vim.keymap.set("t", M.config.toggle_key, M.hide, { buffer = state.buf, desc = "Hide gclic" })
  end
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
