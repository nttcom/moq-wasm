if redis.call('HEXISTS', KEYS[1], ARGV[1]) == 1 then
    return 0
end
redis.call('HSET', KEYS[1], ARGV[1], 'active')
redis.call('EXPIRE', KEYS[1], tonumber(ARGV[2]))
return 1
