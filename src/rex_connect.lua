-- rexue-connect: validate in Rust, then commit on this same CLI connection.
local response = snapshot()
response.connection = {previous = rex.session_id, inspections = {}}
if not rex.args.detach then
    local listed, err = rex.call("client.list", {kinds = {"app"}})
    assert(listed, err)
    response.connection.clients = listed.clients or {}
    for _, client in ipairs(listed.clients or {}) do
        if client.info.kind == "app" and client.connection_state == "connected" then
            local inspected, err = rex.call("client.inspect", {client_id = client.client_id})
            response.connection.inspections[client.client_id] = inspected or {error = tostring(err)}
        end
    end
end
io.write(rex.json.encode(response), "\n")
io.flush()
local line = io.read("*l")
assert(line, "connection cancelled before target validation")
local selected = rex.json.decode(line)
local args = {session_id = selected.session_id}
if selected.block_id then
    args.block_id = selected.block_id
    local result, err = rex.call("session.focus_block", args)
    assert(result, err)
elseif selected.window_id then
    args.window_id = selected.window_id
    local result, err = rex.call("session.focus_window", args)
    assert(result, err)
end
if selected.client_id then
    local args = {session_id = selected.session_id, window_id = selected.window_id}
    local result, err = rex.call("client.perform", {
        client_id = selected.client_id, action = "session.select", args = args, origin = "cli",
    })
    assert(result, err)
end
return {}
