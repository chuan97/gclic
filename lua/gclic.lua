-- gclic in Neovim: gclic in a floating window, toggled with a key, staying
-- alive while hidden, so it's in the same state each time it comes back.
--
-- gclic finds the Neovim it runs in through $NVIM. Its `o` calls `open` below,
-- which hides gclic and opens the file at the line in the window beneath, and
-- its `:cd` changes Neovim's working folder, so the file finder, search and
-- file tree follow the repo picked in gclic.
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
  -- The window's share of the editor, wide and high.
  width = 0.9,
  height = 0.9,
  border = "rounded",
  -- A key that hides gclic from inside it too, where its keys are gclic's.
  -- One gclic doesn't use. False for none.
  toggle_key = "<C-g>",
}

local state = { buf = nil, win = nil }

local function valid_win()
  return state.win ~= nil and vim.api.nvim_win_is_valid(state.win)
end

local function valid_buf()
  return state.buf ~= nil and vim.api.nvim_buf_is_valid(state.buf)
end

local function show()
  local columns, lines = vim.o.columns, vim.o.lines
  local width = math.floor(columns * M.config.width)
  local height = math.floor(lines * M.config.height)
  state.win = vim.api.nvim_open_win(state.buf, true, {
    relative = "editor",
    width = width,
    height = height,
    col = math.floor((columns - width) / 2),
    row = math.floor((lines - height) / 2),
    style = "minimal",
    border = M.config.border,
  })
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
  if vim.fn.has("nvim-0.11") == 1 then
    vim.fn.jobstart({ M.config.cmd }, { term = true, on_exit = on_exit })
  else
    vim.fn.termopen({ M.config.cmd }, { on_exit = on_exit })
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
end

return M
