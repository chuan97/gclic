-- Installing gclic for the plugin: the release build matching the plugin's
-- version, from GitHub, into Neovim's data folder, where the plugin finds it
-- before any gclic on the PATH. lazy.nvim runs it on install and update,
-- through build.lua at the repo's root; `:GclicInstall` runs it otherwise.
--
-- The plugin's version is the one in the Cargo.toml beside it, as the plugin
-- is gclic's repo: a release tag's is that release's.

local M = {}

-- Where the releases are, each in a folder named by its tag. Tests point it
-- at a folder of their own, as a file:// URL.
M.releases = "https://github.com/chuan97/gclic/releases/download"

--- The plugin's folder: the repo's root, two up from this file.
local function root()
  local here = debug.getinfo(1, "S").source:sub(2)
  return vim.fn.fnamemodify(here, ":p:h:h:h")
end

--- The plugin's version, from its Cargo.toml.
function M.version()
  local ok, lines = pcall(vim.fn.readfile, root() .. "/Cargo.toml")
  if not ok then
    return nil
  end
  for _, line in ipairs(lines) do
    local version = line:match('^version%s*=%s*"([^"]+)"')
    if version then
      return version
    end
  end
end

local function windows()
  return vim.fn.has("win32") == 1
end

--- The release's name for this machine, as dist builds them, or nil and why.
function M.target()
  local uname = vim.uv.os_uname()
  local machine = ({ arm64 = "aarch64", aarch64 = "aarch64", x86_64 = "x86_64", AMD64 = "x86_64" })[uname.machine]
  if not machine then
    return nil, "no release for this processor, " .. uname.machine
  end
  if uname.sysname == "Darwin" then
    return machine .. "-apple-darwin"
  elseif uname.sysname == "Linux" then
    return machine .. "-unknown-linux-gnu"
  elseif windows() and machine == "x86_64" then
    return "x86_64-pc-windows-msvc"
  end
  return nil, "no release for " .. uname.sysname .. " on " .. uname.machine
end

--- Where the plugin's gclic goes.
function M.dir()
  return vim.fn.stdpath("data") .. "/gclic"
end

function M.binary()
  return M.dir() .. "/bin/gclic" .. (windows() and ".exe" or "")
end

--- The version of the gclic installed, as it says, or nil.
function M.installed_version()
  if vim.fn.executable(M.binary()) ~= 1 then
    return nil
  end
  local result = vim.system({ M.binary(), "--version" }, { text = true }):wait(5000)
  return result.code == 0 and (result.stdout or ""):match("^gclic (%S+)") or nil
end

local function run(args, what)
  local result = vim.system(args, { text = true }):wait(120000)
  if result.code ~= 0 then
    error(what .. ": " .. vim.trim((result.stderr or "") .. (result.stdout or "")), 0)
  end
end

--- Downloads and unpacks the release, unless it's in already. Errors say what
--- went wrong, for lazy.nvim to show.
function M.install(opts)
  opts = opts or {}
  local say = opts.say or print
  local version = M.version()
  if not version then
    error("can't tell the plugin's version, from its Cargo.toml", 0)
  end
  if not opts.force and M.installed_version() == version then
    say("gclic " .. version .. " is installed")
    return
  end
  local target, why = M.target()
  if not target then
    error(why .. ": build gclic from source, and set the plugin's `cmd`", 0)
  end
  for _, tool in ipairs({ "curl", "tar" }) do
    if vim.fn.executable(tool) ~= 1 then
      error("installing gclic needs " .. tool .. " on the PATH", 0)
    end
  end
  local archive = "gclic-" .. target .. (windows() and ".zip" or ".tar.gz")
  local url = M.releases .. "/v" .. version .. "/" .. archive
  local work = vim.fn.tempname()
  vim.fn.mkdir(work, "p")
  say("Downloading gclic " .. version .. " for " .. target)
  run({ "curl", "--fail", "--location", "--silent", "--show-error", "--output", work .. "/" .. archive, url },
    "downloading " .. url)
  -- tar unpacks zips too on Windows, where it's bsdtar.
  run({ "tar", "-xf", work .. "/" .. archive, "-C", work }, "unpacking " .. archive)
  local name = "gclic" .. (windows() and ".exe" or "")
  local found = vim.fs.find(name, { path = work, type = "file", limit = 1 })[1]
  if not found then
    error(archive .. " has no " .. name, 0)
  end
  vim.fn.mkdir(M.dir() .. "/bin", "p")
  vim.fn.delete(M.binary())
  assert(vim.uv.fs_rename(found, M.binary()))
  if not windows() then
    vim.uv.fs_chmod(M.binary(), tonumber("755", 8))
  end
  vim.fn.delete(work, "rf")
  say("Installed gclic " .. version .. " at " .. M.binary())
end

return M
