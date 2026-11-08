-- Rex init.lua fragment: Alt+S opens or focuses rexue over the invoking tab.
-- Official APIs:
-- https://www.superlogical.com/rex/docs/reference/lua.md
-- Validate: rex config check examples/rex-picker-floating.lua
-- shortcut: renaming the picker pane bypasses reuse; use block tags when Rex exposes them.
local picker_label = "Rex Session Picker"

local function find_session_picker(window_id)
  local view, err = rex.session.view{}
  assert(view, err)
  for _, window in ipairs(view.windows or {}) do
    if window.window_id == window_id then
      for _, layer in ipairs(window.layers or {}) do
        if layer.kind == "floating" then
          for _, block in ipairs(layer.blocks or {}) do
            if block.label == picker_label and block.creator_name == "com.superlogical.terminal" then
              return { layer_id = layer.layer_id, block_ids = { block.block_id }, revision = view.revision }
            end
          end
        end
      end
    end
  end
end

local function focus_picker(picker)
  local focused, err = rex.session.focus_block{ block_id = picker.block_ids[1] }
  assert(focused, err)
  return picker
end

rex.action{
  name = "rexue_picker",
  title = "Open Rex Session Picker",
  run = function(ctx)
    assert(ctx.client_id, "Open the picker from a Rex app client")
    assert(ctx.block_id, "Open the picker from a terminal pane")
    local target, err = rex.session.describe_block{ block_id = ctx.block_id }
    assert(target, err)
    local window_id = assert(target.block.window_id, "The picker terminal is not in a tab")
    local existing = find_session_picker(window_id)
    if existing then return focus_picker(existing) end
    local layer, layer_err = rex.session.new_layer{
      window_id = window_id,
      bounds = { 0.25, 0.3, 0.5, 0.4 },
      focus = true,
      layout = rex.layout.block{
        flavor = "com.superlogical.terminal.shell",
        label = picker_label,
        options = {
          command = { "rexue", "--client", ctx.client_id, "picker", "--focus-pane", ctx.block_id },
        },
      },
    }
    assert(layer, layer_err)
    -- Concurrent commands keep the first committed layer and close only their own duplicate.
    local first = find_session_picker(window_id)
    if first and first.layer_id ~= layer.layer_id then
      local closed, close_err = rex.session.close_layer{ layer_id = layer.layer_id }
      assert(closed, close_err)
      return focus_picker(first)
    end
    return layer
  end,
}

rex.bind("alt+s", "rexue_picker")
