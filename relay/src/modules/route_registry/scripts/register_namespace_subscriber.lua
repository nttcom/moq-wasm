if redis.call('HEXISTS', KEYS[1], ARGV[1]) == 1 then
    return 0
end
redis.call('HSET', KEYS[1], ARGV[1], 'active')
redis.call('EXPIRE', KEYS[1], tonumber(ARGV[2]))
redis.call('SADD', KEYS[2], KEYS[1])
redis.call('EXPIRE', KEYS[2], tonumber(ARGV[3]))
return 1
