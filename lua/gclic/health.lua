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
end

return M
