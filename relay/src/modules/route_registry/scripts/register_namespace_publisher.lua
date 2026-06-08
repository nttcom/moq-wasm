if ARGV[2] == 'active' then
    local entries = redis.call('HGETALL', KEYS[1])
    for i = 1, #entries, 2 do
        if entries[i] ~= ARGV[1] and entries[i+1] == 'active' then
            return 0
        end
    end
end
redis.call('HSET', KEYS[1], ARGV[1], ARGV[2])
redis.call('EXPIRE', KEYS[1], tonumber(ARGV[3]))
redis.call('SADD', KEYS[2], KEYS[1])
redis.call('EXPIRE', KEYS[2], tonumber(ARGV[4]))
return 1
