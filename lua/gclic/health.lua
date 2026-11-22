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
  local cmd = gclic.config.cmd
  local path = vim.fn.exepath(cmd)
  if path == "" then
    health.error("can't find `" .. cmd .. "`", {
      "Install gclic: see https://github.com/chuan97/gclic#installation",
      "Or set the plugin's `cmd` option to where gclic is",
    })
    return
  end
  -- An older gclic, without --version, would start instead: the wait ends it.
  local result = vim.system({ path, "--version" }, { text = true }):wait(2000)
  local installed = result.code == 0 and (result.stdout or ""):match("^gclic (%S+)") or nil
  if not installed then
    health.warn(path .. " is too old to say its version", {
      "Update gclic, as the plugin may call on things it doesn't have",
    })
  else
    health.ok("gclic " .. installed .. " at " .. path)
  end

  if vim.fn.executable("git") == 1 then
    health.ok("git at " .. vim.fn.exepath("git"))
  else
    health.error("gclic needs git on the PATH")
  end
end

return M
