-- Rex init.lua fragment: Cmd+Shift+P opens rexue in a new terminal tab.
-- Official APIs:
-- https://www.superlogical.com/rex/docs/customize/keyboard-shortcuts.md
-- https://www.superlogical.com/rex/docs/reference/lua.md
-- Validate this file without installing or reloading it:
--   rex config check examples/rex-picker.lua
-- Validation checks config syntax; invoking the action requires a running Rex
-- client and rexue on the terminal's PATH. No personal config is modified.
rex.action{
  name = "rexue_picker",
  title = "Open Rex Session Picker",
  run = function(ctx, args)
    assert(ctx.client_id, "Open the picker from a Rex app client")
    local window, err = rex.session.new_window{
      layout = rex.layout.block{
        flavor = "com.superlogical.terminal.shell",
        options = {
          command = { "rexue", "--client", ctx.client_id },
        },
      },
    }
    assert(window, err)
    return window
  end,
}

rex.bind("cmd+shift+p", "rexue_picker")
