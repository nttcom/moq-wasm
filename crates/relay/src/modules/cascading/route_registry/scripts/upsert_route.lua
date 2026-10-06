redis.call('HSET', KEYS[1], ARGV[1], 'active')
redis.call('EXPIRE', KEYS[1], tonumber(ARGV[2]))
return 1
