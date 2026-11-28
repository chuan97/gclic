-- lazy.nvim runs this on installing and updating the plugin: it fetches the
-- gclic release matching the plugin's version, for the plugin to run. See
-- lua/gclic/install.lua.
require("gclic.install").install()
