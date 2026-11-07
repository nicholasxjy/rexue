-- Keep discovery on one documented Rex CLI connection. Arguments carry IDs as data.
local listed, err = rex.call("session.list")
if not listed then error(err) end
local snapshots = {}
for _, session in ipairs(listed.sessions or {}) do
    local attached, err = rex.call("session.attach", {session_id = session.session_id})
    if not attached then
        if tostring(err):find("code=session_not_found", 1, true) then
            snapshots[session.session_id] = {missing = true}
        else
            error(err)
        end
    else
        local blocks = {}
        for _, window in ipairs(attached.view.windows or {}) do
            for _, layer in ipairs(window.layers or {}) do
                for _, block in ipairs(layer.blocks or {}) do
                    local picker = layer.kind == "floating" and block.block_id == rex.args.picker_block
                    if block.creator_name == "com.superlogical.terminal" and not picker then
                        local metadata = {}
                        for _, method in ipairs({"process", "program_status", "title"}) do
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
        snapshots[session.session_id] = {view = attached.view, blocks = blocks}
    end
end
return {sessions = listed.sessions or {}, snapshots = snapshots}
