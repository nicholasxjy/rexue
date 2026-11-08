-- Keep discovery on one documented Rex CLI connection. Arguments carry IDs as data.
local listed, err
if rex.args.session_id then
    listed = {sessions = {{session_id = rex.args.session_id}}}
else
    listed, err = rex.call("session.list")
    if not listed then error(err) end
end
local snapshots = {}
local target_only = rex.args.connect and rex.args.block_id ~= nil
local methods = rex.args.connect and {"process", "program_status"} or {"process", "program_status", "title"}
for _, session in ipairs(listed.sessions or {}) do
    local attached, err = rex.call("session.attach", {session_id = session.session_id})
    if not attached then
        if tostring(err):find("code=session_not_found", 1, true) then
            snapshots[session.session_id] = {missing = true}
        else
            error(err)
        end
    else
        session.label = session.label or attached.view.label
        local blocks = {}
        for _, window in ipairs(attached.view.windows or {}) do
            for _, layer in ipairs(window.layers or {}) do
                for _, block in ipairs(layer.blocks or {}) do
                    local picker = layer.kind == "floating" and block.block_id == rex.args.picker_block
                    local selected = not rex.args.block_id or block.block_id == rex.args.block_id
                        or (not target_only and block.block_id == window.focused_block_id)
                    if block.creator_name == "com.superlogical.terminal" and not picker and selected then
                        local metadata = {}
                        for _, method in ipairs(methods) do
                            -- Optional terminal methods may fail for an exited pane or
                            -- an older terminal. Process identity stays authoritative.
                            metadata[method] = rex.call("block.method", {
                                session_id = session.session_id,
                                block_id = block.block_id,
                                method = method,
                            })
                        end
                        blocks[block.block_id] = metadata
                    end
                end
            end
        end
        snapshots[session.session_id] = {view = attached.view, blocks = blocks, target_only = target_only or false}
    end
end
return {sessions = listed.sessions or {}, snapshots = snapshots}
