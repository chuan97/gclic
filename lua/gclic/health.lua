-- :checkhealth gclic: whether the plugin has what it needs.

local M = {}

function M.check()
  local health = vim.health
  local gclic = require("gclic")

  health.start("Neovim")
  if vim.fn.has("nvim-0.10") == 1 then
    health.ok("Neovim " .. tostring(vim.version()))
  else
    health.error("gclic's plugin needs Neovim 0.10 or later")
  end

  health.start("gclic")
  local install = require("gclic.install")
  local cmd = gclic.command()
  local from = gclic.config.cmd and "your `cmd`"
    or (cmd == install.binary() and "installed by the plugin" or "on the PATH")
  local path = vim.fn.exepath(cmd)
  if path == "" then
    health.error("can't find gclic", {
      "Run :GclicInstall, for the release matching the plugin",
      "Or install gclic: see https://github.com/chuan97/gclic#installation",
      "Or set the plugin's `cmd` option to where gclic is",
    })
    return
  end
  -- An older gclic, without --version, would start instead: the wait ends it.
  local result = vim.system({ path, "--version" }, { text = true }):wait(2000)
  local installed = result.code == 0 and (result.stdout or ""):match("^gclic (%S+)") or nil
  local wanted = install.version()
  if not installed then
    health.warn(path .. " is too old to say its version", {
      "Run :GclicInstall, or update gclic, as the plugin may call on things it doesn't have",
    })
  elseif wanted and installed ~= wanted then
    health.warn("gclic " .. installed .. " at " .. path .. ", " .. from .. ", but the plugin is " .. wanted, {
      "Run :GclicInstall for the gclic matching the plugin, or update the one you run",
    })
  else
    health.ok("gclic " .. installed .. " at " .. path .. ", " .. from)
  end

  if vim.fn.executable("git") == 1 then
    health.ok("git at " .. vim.fn.exepath("git"))
  else
    health.error("gclic needs git on the PATH")
  end

  health.start("Sidebar and file tree")
  local config = gclic.config
  if type(config.sidebar) == "table" then
    health.ok("gclic opens in a sidebar at the left, " .. (config.sidebar.width or 40) .. " columns wide")
  else
    health.ok("gclic opens over the whole editor, without the sidebar (`sidebar = false`)")
  end
  local status = gclic._file_tree_status()
  if status.tree then
    local key = config.file_tree_key and (config.file_tree_key .. " in gclic opens " .. status.tree) or status.tree
    health.ok(key)
  elseif config.file_tree_key then
    health.warn("no file tree found, for " .. config.file_tree_key .. " in gclic to open", {
      "Install Snacks's explorer, neo-tree or nvim-tree, or set the plugin's `file_tree` to a function",
    })
  end
  if status.tree and config.close_file_tree and type(config.sidebar) == "table" then
    if status.seen then
      health.ok("the sidebar and the file tree take turns at the left")
    else
      health.warn("this Snacks doesn't list its open pickers as gclic expects", {
        "The sidebar and the explorer won't take turns: update Snacks, or set `close_file_tree = false`",
      })
    end
  end
end

return M
