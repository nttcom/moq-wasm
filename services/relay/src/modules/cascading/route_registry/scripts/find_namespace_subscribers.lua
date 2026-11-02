local self_relay_id = ARGV[1]
local result = {}
-- A relay can register under multiple overlapping prefixes (e.g. both "a" and "a/b").
-- Since we scan all prefix levels of the queried namespace, the same relay may appear
-- in more than one KEYS entry, so we deduplicate by relay_id here.
local seen = {}

for i = 1, #KEYS do
    local entries = redis.call('HGETALL', KEYS[i])
    for j = 1, #entries, 2 do
        local relay_id = entries[j]
        local status = entries[j + 1]
        if relay_id ~= self_relay_id and status == 'active' and not seen[relay_id] then
            local relay_info = redis.call('HGETALL', 'relay:' .. relay_id)
            if #relay_info == 0 then
                redis.call('HDEL', KEYS[i], relay_id)
            else
                local r_status, r_host, r_port
                for k = 1, #relay_info, 2 do
                    if relay_info[k] == 'status' then r_status = relay_info[k + 1]
                    elseif relay_info[k] == 'host' then r_host = relay_info[k + 1]
                    elseif relay_info[k] == 'port' then r_port = relay_info[k + 1]
                    end
                end
                if r_status == 'active' and r_host and r_port then
                    seen[relay_id] = true
                    result[#result + 1] = relay_id
                    result[#result + 1] = r_host
                    result[#result + 1] = r_port
                    result[#result + 1] = r_status
                end
            end
        end
    end
end

return result
