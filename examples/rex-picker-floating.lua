-- Rex init.lua fragment: Alt+S opens rexue over the invoking terminal's tab.
-- Official APIs:
-- https://www.superlogical.com/rex/docs/reference/lua.md
-- Inspect window targeting: rex api describe session.new_layer
-- Validate without installing or reloading:
--   rex config check examples/rex-picker-floating.lua
rex.action{
  name = "rexue_picker",
  title = "Open Rex Session Picker",
  run = function(ctx)
    assert(ctx.client_id, "Open the picker from a Rex app client")
    assert(ctx.block_id, "Open the picker from a terminal pane")
    local target, err = rex.session.describe_block{ block_id = ctx.block_id }
    assert(target, err)
    local layer, layer_err = rex.session.new_layer{
      window_id = assert(target.block.window_id, "The picker terminal is not in a tab"),
      bounds = { 0.25, 0.3, 0.5, 0.4 },
      focus = true,
      layout = rex.layout.block{
        flavor = "com.superlogical.terminal.shell",
        options = {
          command = { "rexue", "--client", ctx.client_id, "picker", "--focus-pane", ctx.block_id },
        },
      },
    }
    assert(layer, layer_err)
    return layer
  end,
}

rex.bind("alt+s", "rexue_picker")
