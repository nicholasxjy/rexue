-- Run from the repository root: rex do scripts/shortcut_smoke.lua
local original_rex = rex
local fixture = { created = {}, focused = {}, blocks = {}, windows = {} }
for _, name in ipairs({ "a", "b", "c", "d" }) do
  local window = "window:" .. name .. "-full"
  fixture.windows[#fixture.windows + 1] = window
  fixture.blocks["block:origin-" .. name .. "-full"] = { window_id = window }
end
rex = {
  action = function(spec) fixture.action = spec.run end,
  bind = function() end,
  layout = { block = function(options) return { block = options } end },
  session = {
    describe_block = function(args)
      local block = fixture.blocks[args.block_id]
      if not block then return nil, "not_found" end
      return { block = { block_id = args.block_id, window_id = block.window_id } }
    end,
    view = function()
      if fixture.fail_view then return nil, "fixture view failure" end
      local result = { revision = #fixture.created, windows = {} }
      for _, id in ipairs(fixture.windows) do
        local window = { window_id = id, layers = {} }
        for _, item in ipairs(fixture.created) do
          if not item.closed and item.args.window_id == id then
            window.layers[#window.layers + 1] = {
              kind = "floating", layer_id = item.layer.layer_id,
              blocks = { { block_id = item.layer.block_ids[1],
                label = item.args.layout.block.label, creator_name = "com.superlogical.terminal" } },
            }
          end
        end
        result.windows[#result.windows + 1] = window
      end
      return result
    end,
    focus_block = function(args)
      assert(fixture.blocks[args.block_id], "cannot focus a closed picker")
      fixture.focused[#fixture.focused + 1] = args.block_id
      return { revision = #fixture.focused }
    end,
    close_layer = function(args)
      for _, item in ipairs(fixture.created) do
        if item.layer.layer_id == args.layer_id then
          item.closed = true
          fixture.blocks[item.layer.block_ids[1]] = nil
          return { revision = #fixture.created }
        end
      end
      return nil, "not_found"
    end,
    new_layer = function(args)
      if fixture.fail_next then
        fixture.fail_next = false
        return nil, "fixture creation failure"
      end
      if fixture.on_create then
        local callback = fixture.on_create
        fixture.on_create = nil
        callback()
      end
      local id = #fixture.created + 1
      local layer = { layer_id = "layer:picker-" .. id, block_ids = { "block:picker-" .. id } }
      fixture.blocks[layer.block_ids[1]] = { window_id = args.window_id }
      fixture.created[id] = { layer = layer, args = args }
      return layer
    end,
  },
}
local function context(name)
  return { client_id = "client:desktop-full", block_id = "block:origin-" .. name .. "-full" }
end
local function invoke(ctx)
  -- Reload the callback on every invocation, as separate Rex commands do.
  dofile("examples/rex-picker-floating.lua")
  return fixture.action(ctx)
end
local function active(window)
  local count = 0
  for _, item in ipairs(fixture.created) do
    if not item.closed and item.args.window_id == window and item.args.layout.block.label == "Rex Session Picker" then count = count + 1 end
  end
  return count
end
local a, b, c, d = context("a"), context("b"), context("c"), context("d")
local first = invoke(a)
assert(fixture.created[1].args.window_id == "window:a-full", "use the invoking pane's tab")
assert(fixture.created[1].args.layout.block.options.command[6] == a.block_id, "preserve the invoking pane's full ID")
assert(invoke(a).layer_id == first.layer_id, "reuse across separate command invocations")
assert(#fixture.created == 1, "repeated commands created another picker")
local picker_context = { client_id = a.client_id, block_id = first.block_ids[1] }
assert(invoke(picker_context).layer_id == first.layer_id, "reopening from the picker must reuse it")
assert(fixture.focused[#fixture.focused] == first.block_ids[1], "focus by full block ID")
local unrelated = rex.session.new_layer{
  window_id = "window:a-full", layout = { block = { label = "Other floating terminal" } },
}
assert(invoke(a).layer_id == first.layer_id, "ignore unrelated floating panes")
assert(fixture.blocks[unrelated.block_ids[1]], "preserve unrelated floating panes")
local other = invoke(b)
assert(active("window:a-full") == 1 and active("window:b-full") == 1, "tabs remain independent")
rex.session.close_layer{ layer_id = first.layer_id }
local reopened = invoke(a)
assert(reopened.layer_id ~= first.layer_id and active("window:a-full") == 1, "reopen after closing")
fixture.fail_next = true
local ok, err = pcall(invoke, c)
assert(not ok and tostring(err):find("fixture creation failure", 1, true), "preserve creation failures")
invoke(c)
assert(active("window:c-full") == 1, "retry after failed creation")
fixture.on_create = function() invoke(d) end
local winner = invoke(d)
assert(active("window:d-full") == 1, "concurrent opens must leave one picker")
assert(fixture.blocks[winner.block_ids[1]], "return the surviving full block ID")
assert(not fixture.created[#fixture.created - 1].closed, "preserve the first committed picker")
assert(fixture.created[#fixture.created].closed, "close only this invocation's duplicate")
fixture.fail_view = true
local before = #fixture.created
ok, err = pcall(invoke, a)
assert(not ok and tostring(err):find("fixture view failure", 1, true), "preserve discovery failures")
assert(#fixture.created == before, "failed discovery must not create a picker")
fixture.fail_view = false
assert(invoke(a).layer_id == reopened.layer_id, "retry discovery without duplicating")
rex = original_rex
print("PASS: independent invocations, focus, tab isolation, close, failure recovery and concurrent opens")
