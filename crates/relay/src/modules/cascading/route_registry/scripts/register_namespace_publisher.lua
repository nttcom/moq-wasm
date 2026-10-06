local entries = redis.call('HGETALL', KEYS[1])
for i = 1, #entries, 2 do
    if entries[i] ~= ARGV[1] and entries[i+1] == 'active' then
        return 0
    end
end
redis.call('HSET', KEYS[1], ARGV[1], 'active')
redis.call('EXPIRE', KEYS[1], tonumber(ARGV[2]))
return 1
